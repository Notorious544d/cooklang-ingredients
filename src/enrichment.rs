use crate::loader::{self, Meta, Product};
use anyhow::Context;
use cooklang::{
    Converter, Recipe,
    convert::{ConvertTo, ConvertUnit, ConvertValue},
    quantity::Value,
};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

pub type EnrichedIngredients = Vec<EnrichedIngredient>;

#[derive(Debug, Serialize)]
pub struct EnrichedIngredient {
    pub name: String,
    pub products: Vec<Product>,
    pub occurrences: Vec<IngredientOccurrence>,
    pub source: String,  // "local", "central", or "unresolved"
}

#[derive(Debug, Serialize)]
pub struct IngredientOccurrence {
    pub recipe_slug: String,
    pub quantity_g: Option<f64>,
    pub scalable: bool,
}

pub fn resolve_recipe_ingredients(
    root_slug: &str,
    recipes: &HashMap<String, Recipe>,
    ingredient_data_dir: &Path,
) -> anyhow::Result<EnrichedIngredients> {
    let mut result = Vec::new();
    let mut ingredient_indices = HashMap::new();
    let mut stack = Vec::new();
    let converter = Converter::default();

    visit_recipe(
        &recipe_key(root_slug),
        1.0,
        recipes,
        ingredient_data_dir,
        &converter,
        &mut stack,
        &mut ingredient_indices,
        &mut result,
        false,
    )?;

    Ok(result)
}

fn visit_recipe(
    slug: &str,
    multiplier: f64,
    recipes: &HashMap<String, Recipe>,
    ingredient_data_dir: &Path,
    converter: &Converter,
    stack: &mut Vec<String>,
    ingredient_indices: &mut HashMap<String, usize>,
    output: &mut EnrichedIngredients,
    is_linked: bool,
) -> anyhow::Result<()> {
    if stack.iter().any(|item| item == slug) {
        let mut cycle = stack.clone();
        cycle.push(slug.to_owned());
        anyhow::bail!("linked recipe cycle: {}", cycle.join(" -> "));
    }

    let recipe = recipes
        .get(slug)
        .with_context(|| format!("linked recipe '{slug}' was not found"))?;

    stack.push(slug.to_owned());

    for item in &recipe.ingredients {
        if let Some(reference) = &item.reference {
            let child_slug = recipe_key(&reference.name);
            // Assumption: `{recipe}` means one child batch following the parent scale.
            // Revisit if Cooklang's recipe-reference semantics change.
            let child_multiplier = reference_multiplier(item)?;

            visit_recipe(
                &child_slug,
                multiplier * child_multiplier,
                recipes,
                ingredient_data_dir,
                converter,
                stack,
                ingredient_indices,
                output,
                true,
            )?;
            continue;
        }

        let name = item.name.trim();
        let key = name.to_lowercase();

        let index = if let Some(&index) = ingredient_indices.get(&key) {
            index
        } else {
            // Load meta and products
            let (meta, source_products) =
                match loader::load_ingredient_data(ingredient_data_dir, name)? {
                    Some(data) => data,
                    None => {
                        record_unresolved(
                            output,
                            name,
                            slug,
                            "ingredient data not found".to_owned(),
                        );
                        continue;
                    }
                };

            let products = source_products
                .into_iter()
                .map(|source| enrich_product(source, &meta, converter))
                .collect::<anyhow::Result<Vec<_>>>()?;

            let index = output.len();
            output.push(EnrichedIngredient {
                name: name.to_owned(),
                products,
                occurrences: Vec::new(),
                source: "local".to_owned(),
            });
            ingredient_indices.insert(key, index);
            index
        };

        // Now calculate quantity using meta from the ingredient we just created
        let quantity_g = match item.quantity.as_ref() {
            Some(quantity) => {
                let (meta, _) = loader::load_ingredient_data(ingredient_data_dir, name)?
                    .ok_or_else(|| anyhow::anyhow!("meta disappeared for {}", name))?;
                match quantity_to_grams(
                    quantity,
                    &meta,
                    multiplier,
                    converter,
                ) {
                    Ok(grams) => Some(grams),
                    Err(error) => {
                        record_unresolved(output, name, slug, error.to_string());
                        None
                    }
                }
            }
            None => {
                record_unresolved(output, name, slug, "quantity is missing".to_owned());
                None
            }
        };

        output[index]
            .occurrences
            .push(IngredientOccurrence {
                recipe_slug: slug.to_owned(),
                quantity_g,
                scalable: !is_linked,
            });
    }

    stack.pop();
    Ok(())
}

fn quantity_to_grams(
    quantity: &cooklang::Quantity,
    meta: &Meta,
    multiplier: f64,
    converter: &Converter,
) -> anyhow::Result<f64> {
    let value = match quantity.value() {
        Value::Number(number) => number.value(),
        Value::Range { .. } => anyhow::bail!("quantity is a range"),
        Value::Text(_) => anyhow::bail!("quantity is text"),
    };
    anyhow::ensure!(value.is_finite() && value >= 0.0, "quantity is invalid");
    anyhow::ensure!(
        multiplier.is_finite() && multiplier >= 0.0,
        "recipe multiplier is invalid"
    );

    let unit = quantity
        .unit()
        .context("quantity has no unit; cannot infer its size")?;
    let amount_g = if let Some(unit_size) = meta
        .unit_size
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, size)| *size)
    {
        amount_in_unit_to_grams(value * unit_size, &meta.default_unit, meta, converter)?
    } else {
        match convert(value, unit, "g", converter) {
            Ok(amount) => amount,
            Err(_) => {
                let amount_ml = convert(value, unit, "ml", converter)
                    .with_context(|| format!("cannot convert '{unit}' to grams or millilitres"))?;
                let density = meta
                    .density
                    .context("ingredient density is required to convert volume to grams")?;
                anyhow::ensure!(
                    density.is_finite() && density > 0.0,
                    "density must be positive"
                );
                amount_ml * density
            }
        }
    };

    let amount_g = amount_g * multiplier;
    anyhow::ensure!(amount_g.is_finite(), "calculated quantity is not finite");
    Ok(amount_g)
}

fn amount_in_unit_to_grams(
    value: f64,
    unit: &str,
    meta: &Meta,
    converter: &Converter,
) -> anyhow::Result<f64> {
    match convert(value, unit, "g", converter) {
        Ok(amount) => Ok(amount),
        Err(_) => {
            let amount_ml = convert(value, unit, "ml", converter)
                .with_context(|| format!("default unit '{}' is not convertible", unit))?;
            let density = meta
                .density
                .context("ingredient density is required to convert volume to grams")?;
            anyhow::ensure!(
                density.is_finite() && density > 0.0,
                "density must be positive"
            );
            Ok(amount_ml * density)
        }
    }
}

fn convert(value: f64, from: &str, to: &str, converter: &Converter) -> anyhow::Result<f64> {
    let (converted, _) = converter
        .convert(
            ConvertValue::Number(value),
            ConvertUnit::Key(from),
            ConvertTo::Unit(ConvertUnit::Key(to)),
        )
        .with_context(|| format!("converting {value} {from} to {to}"))?;

    match converted {
        ConvertValue::Number(value) => Ok(value),
        ConvertValue::Range(_) => unreachable!("numeric conversion returned a range"),
    }
}

fn enrich_product(
    mut source: Product,
    meta: &Meta,
    converter: &Converter,
) -> anyhow::Result<Product> {
    let package_g = amount_in_unit_to_grams(source.quantity, &meta.default_unit, meta, converter)?;

    anyhow::ensure!(
        package_g.is_finite() && package_g > 0.0,
        "product '{}' must have a positive package quantity",
        source.name
    );

    let cheapest_price = source
        .stores
        .iter()
        .flat_map(|(store, prices)| prices.iter().map(move |(date, price)| (date, store, price)))
        .min_by(|(_, _, price_a), (_, _, price_b)| price_a.partial_cmp(price_b).unwrap());

    source.cost_per_100g = cheapest_price
        .map(|(_, _, price)| {
            anyhow::ensure!(
                price.is_finite() && *price >= 0.0,
                "invalid price for '{}'",
                source.name
            );
            Ok(*price / package_g * 100.0)
        })
        .transpose()?;

    Ok(source)
}

fn record_unresolved(
    output: &mut EnrichedIngredients,
    ingredient: &str,
    recipe_slug: &str,
    _reason: String,
) {
    let key = ingredient.to_lowercase();
    let already_exists = output.iter().any(|entry| entry.name.to_lowercase() == key);
    
    if !already_exists {
        output.push(EnrichedIngredient {
            name: ingredient.to_owned(),
            products: Vec::new(),
            occurrences: vec![IngredientOccurrence {
                recipe_slug: recipe_slug.to_owned(),
                quantity_g: None,
                scalable: true,
            }],
            source: "unresolved".to_owned(),
        });
    } else {
        // Add occurrence to existing unresolved ingredient
        if let Some(entry) = output.iter_mut().find(|e| e.name.to_lowercase() == key) {
            entry.occurrences.push(IngredientOccurrence {
                recipe_slug: recipe_slug.to_owned(),
                quantity_g: None,
                scalable: true,
            });
        }
    }
}

fn reference_multiplier(ingredient: &cooklang::Ingredient) -> anyhow::Result<f64> {
    let Some(quantity) = &ingredient.quantity else {
        return Ok(1.0);
    };

    match quantity.value() {
        Value::Text(text) if text.trim().eq_ignore_ascii_case("recipe") => Ok(1.0),
        Value::Number(number) if quantity.unit().is_none() => {
            let multiplier = number.value();
            anyhow::ensure!(
                multiplier > 0.0,
                "linked recipe multiplier must be positive"
            );
            Ok(multiplier)
        }
        _ => anyhow::bail!(
            "unsupported linked-recipe quantity for '{}': {}",
            ingredient.name,
            quantity
        ),
    }
}

fn recipe_key(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase()
}