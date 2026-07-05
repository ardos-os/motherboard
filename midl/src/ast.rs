use std::{error::Error as StdError, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub docs: Vec<String>,
    pub name: String,
    pub items: Vec<ServiceItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceItem {
    TypeAlias(TypeAlias),
    Enum(Enum),
    Store(Store),
    Function(Function),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAlias {
    pub docs: Vec<String>,
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enum {
    pub docs: Vec<String>,
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumVariant {
    pub docs: Vec<String>,
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub docs: Vec<String>,
    pub name: String,
    pub params: Vec<Field>,
    pub returns: Type,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Named(String),
    Array(Box<Type>),
    Tuple(Vec<Type>),
    AnonymousStore(Box<Type>),
    Void,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl Diagnostic {
    pub(crate) fn new(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            message: message.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl StdError for Diagnostic {}
