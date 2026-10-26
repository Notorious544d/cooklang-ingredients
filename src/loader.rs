use anyhow::Context;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct Meta {
    pub default_unit: String,

    #[serde(default)]
    pub density: Option<f64>,

    #[serde(default)]
    pub unit_size: IndexMap<String, f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Nutrition {
    pub calories: f64,
    pub protein: f64,
    pub carbohydrates: f64,
    pub sugar: f64,
    pub fat: f64,
    pub saturated_fat: f64,
    pub monounsaturated_fat: f64,
    pub polyunsaturated_fat: f64,
    pub fibre: f64,
    pub sodium: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Product {
    pub name: String,
    pub quantity: f64,
    pub stores: IndexMap<String, IndexMap<String, f64>>,

    #[serde(default)]
    pub nutrition_per_100g: Option<Nutrition>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_100g: Option<f64>,  // Calculated during enrichment
}

pub fn load_ingredient_data(
    root: &Path,
    ingredient_name: &str,
) -> anyhow::Result<Option<(Meta, Vec<Product>)>> {
    let name_path = Path::new(ingredient_name);

    anyhow::ensure!(
        !ingredient_name.is_empty()
            && name_path.components().count() == 1
            && matches!(
                name_path.components().next(),
                Some(std::path::Component::Normal(_))
            ),
        "invalid ingredient name: {ingredient_name}"
    );

    let dir = root.join(name_path);
    if !dir.is_dir() {
        return Ok(None);
    }

    let meta: Meta = load_yaml(&dir.join("meta.yml"))?;
    let default_nutrition: Nutrition = load_yaml(&dir.join("nutrition.yml"))?;
    let mut products: Vec<Product> = load_yaml(&dir.join("products.yml"))?;

    for product in &mut products {
        if product.nutrition_per_100g.is_none() {
            product.nutrition_per_100g = Some(default_nutrition.clone());
        }
    }

    Ok(Some((meta, products)))
}

fn load_yaml<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}