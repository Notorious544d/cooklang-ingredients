use crate::enrichment::EnrichedIngredients;
use crate::loader::Nutrition;
use std::collections::HashMap;

pub fn calculate_recipe_nutrition(
    enriched: &EnrichedIngredients,
    selected_products: &HashMap<String, usize>,
) -> Nutrition {
    let mut total = Nutrition::default();

    for ingredient in enriched {
        // Don't include this ingredient if any occurrence couldn't be converted.
        let Some(grams) = ingredient
            .occurrences
            .iter()
            .map(|occurrence| occurrence.quantity_g)
            .try_fold(0.0, |sum, amount| amount.map(|grams| sum + grams))
        else {
            continue;
        };

        let selected_index = selected_products
            .get(&ingredient.name.to_lowercase())
            .copied()
            .unwrap_or(0);

        let Some(product) = ingredient.products.get(selected_index) else {
            continue;
        };

        let Some(nutrition) = product.nutrition_per_100g.as_ref() else {
            continue;
        };

        add_scaled_nutrition(&mut total, nutrition, grams / 100.0);
    }

    total
}

fn add_scaled_nutrition(total: &mut Nutrition, nutrition: &Nutrition, factor: f64) {
    total.calories += nutrition.calories * factor;
    total.protein += nutrition.protein * factor;
    total.carbohydrates += nutrition.carbohydrates * factor;
    total.sugar += nutrition.sugar * factor;
    total.fat += nutrition.fat * factor;
    total.saturated_fat += nutrition.saturated_fat * factor;
    total.monounsaturated_fat += nutrition.monounsaturated_fat * factor;
    total.polyunsaturated_fat += nutrition.polyunsaturated_fat * factor;
    total.fibre += nutrition.fibre * factor;
    total.sodium += nutrition.sodium * factor;
}
