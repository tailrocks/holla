use std::{future::Future, pin::Pin};

pub type ActionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Danger {
    Safe,
    Mutating,
    Destructive,
}

pub struct ActionSpec {
    pub id: String,
    pub label: String,
    pub description: String,
    pub preview: String,
    pub keywords: Vec<String>,
    pub danger: Danger,
    pub run: Box<dyn Fn() -> ActionFuture + Send>,
}

pub struct GroupSpec {
    pub id: &'static str,
    pub title: String,
    pub actions: Vec<ActionSpec>,
}

impl ActionSpec {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        preview: impl Into<String>,
        keywords: &'static [&'static str],
        danger: Danger,
        run: impl Fn() -> ActionFuture + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            preview: preview.into(),
            keywords: keywords
                .iter()
                .map(|keyword| (*keyword).to_owned())
                .collect(),
            danger,
            run: Box::new(run),
        }
    }
}
