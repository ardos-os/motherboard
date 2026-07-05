use crate::{
    ast::{
        Diagnostic, Document, Enum, EnumVariant, Field, Function, Service, ServiceItem, Store,
        Type, TypeAlias,
    },
    tokenizer::{Token, TokenKind},
};

pub fn parse_tokens(tokens: Vec<Token>) -> Result<Document, Diagnostic> {
    Parser::new(tokens).parse_document()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pending_docs: Vec<String>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_docs: Vec::new(),
        }
    }

    fn parse_document(mut self) -> Result<Document, Diagnostic> {
        let mut services = Vec::new();
        while !self.at_eof() {
            self.collect_docs();
            services.push(self.parse_service()?);
        }
        Ok(Document { services })
    }

    fn parse_service(&mut self) -> Result<Service, Diagnostic> {
        let docs = self.take_docs();
        self.expect_ident_value("service")?;
        let name = self.expect_ident()?;
        self.expect_symbol('{')?;
        let mut items = Vec::new();
        while !self.consume_symbol('}') {
            self.collect_docs();
            if self.consume_separator() {
                continue;
            }
            let docs = self.take_docs();
            let item = if self.consume_ident_value("type") {
                ServiceItem::TypeAlias(self.parse_type_alias(docs)?)
            } else if self.consume_ident_value("enum") {
                ServiceItem::Enum(self.parse_enum(docs)?)
            } else {
                let public = self.consume_ident_value("public");
                if self.consume_ident_value("store") {
                    ServiceItem::Store(self.parse_store(docs, public)?)
                } else if public {
                    return Err(self.error_here("`public` can only modify `store`"));
                } else if self.consume_ident_value("store") {
                    unreachable!()
                } else if self.consume_ident_value("fn") {
                    ServiceItem::Function(self.parse_function(docs)?)
                } else {
                    return Err(self.error_here("expected service item"));
                }
            };
            items.push(item);
            self.consume_separator();
        }
        Ok(Service { docs, name, items })
    }

    fn parse_type_alias(&mut self, docs: Vec<String>) -> Result<TypeAlias, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_symbol('=')?;
        let ty = self.parse_type()?;
        Ok(TypeAlias { docs, name, ty })
    }

    fn parse_enum(&mut self, docs: Vec<String>) -> Result<Enum, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_symbol('{')?;
        let mut variants = Vec::new();
        while !self.consume_symbol('}') {
            self.collect_docs();
            if self.consume_separator() {
                continue;
            }
            let docs = self.take_docs();
            let name = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.consume_symbol('{') {
                while !self.consume_symbol('}') {
                    fields.push(self.parse_field()?);
                    self.consume_separator();
                }
            }
            variants.push(EnumVariant { docs, name, fields });
            self.consume_separator();
        }
        Ok(Enum {
            docs,
            name,
            variants,
        })
    }

    fn parse_store(&mut self, docs: Vec<String>, public: bool) -> Result<Store, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_symbol(':')?;
        let ty = self.parse_type()?;
        Ok(Store {
            docs,
            public,
            name,
            ty,
        })
    }

    fn parse_function(&mut self, docs: Vec<String>) -> Result<Function, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_symbol('(')?;
        let mut params = Vec::new();
        while !self.consume_symbol(')') {
            params.push(self.parse_field()?);
            self.consume_separator();
        }
        self.expect_arrow()?;
        let returns = self.parse_type()?;
        let errors = if self.consume_symbol('?') {
            self.expect_symbol('(')?;
            self.expect_ident_value("error")?;
            self.expect_symbol('{')?;
            let mut errors = Vec::new();
            while !self.consume_symbol('}') {
                errors.push(self.expect_ident()?);
                self.consume_separator();
            }
            self.expect_symbol(')')?;
            errors
        } else {
            Vec::new()
        };

        Ok(Function {
            docs,
            name,
            params,
            returns,
            errors,
        })
    }

    fn parse_field(&mut self) -> Result<Field, Diagnostic> {
        let name = self.expect_ident()?;
        self.expect_symbol(':')?;
        let ty = self.parse_type()?;
        Ok(Field { name, ty })
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        let mut ty = if self.consume_symbol('(') {
            if self.consume_symbol(')') {
                Type::Tuple(Vec::new())
            } else {
                let mut items = Vec::new();
                loop {
                    items.push(self.parse_type()?);
                    if self.consume_symbol(')') {
                        break;
                    }
                    self.expect_symbol(',')?;
                }
                Type::Tuple(items)
            }
        } else {
            let name = self.expect_ident()?;
            if name == "void" {
                Type::Void
            } else if name == "AnonymousStore" {
                self.expect_symbol('<')?;
                let inner = self.parse_type()?;
                self.expect_symbol('>')?;
                Type::AnonymousStore(Box::new(inner))
            } else {
                Type::Named(name)
            }
        };

        while self.consume_symbol('[') {
            self.expect_symbol(']')?;
            ty = Type::Array(Box::new(ty));
        }

        Ok(ty)
    }

    fn collect_docs(&mut self) {
        while let TokenKind::Doc(text) = &self.current().kind {
            self.pending_docs.push(text.clone());
            self.pos += 1;
        }
    }

    fn take_docs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_docs)
    }

    fn consume_separator(&mut self) -> bool {
        self.consume_symbol(';') || self.consume_symbol(',')
    }

    fn consume_symbol(&mut self, symbol: char) -> bool {
        if self.current().kind == TokenKind::Symbol(symbol) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_symbol(&mut self, symbol: char) -> Result<(), Diagnostic> {
        if self.consume_symbol(symbol) {
            Ok(())
        } else {
            Err(self.error_here(format!("expected `{symbol}`")))
        }
    }

    fn expect_arrow(&mut self) -> Result<(), Diagnostic> {
        if self.current().kind == TokenKind::Arrow {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error_here("expected `->`"))
        }
    }

    fn consume_ident_value(&mut self, value: &str) -> bool {
        match &self.current().kind {
            TokenKind::Ident(ident) if ident == value => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_ident_value(&mut self, value: &str) -> Result<(), Diagnostic> {
        if self.consume_ident_value(value) {
            Ok(())
        } else {
            Err(self.error_here(format!("expected `{value}`")))
        }
    }

    fn expect_ident(&mut self) -> Result<String, Diagnostic> {
        match &self.current().kind {
            TokenKind::Ident(ident) => {
                let ident = ident.clone();
                self.pos += 1;
                Ok(ident)
            }
            _ => Err(self.error_here("expected identifier")),
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn error_here(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(message, self.current().line, self.current().column)
    }
}
