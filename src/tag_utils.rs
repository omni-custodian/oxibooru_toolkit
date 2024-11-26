use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Tag {
    pub name: String,
    pub category: String,
    pub aliases: Vec<String>,
    pub implications: Vec<String>,
    pub suggested: Vec<String>,
}