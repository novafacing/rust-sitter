use serde_json::{json, Map, Value};
use std::path::Path;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    *,
};

// TODO(shadaj): share with macro
#[derive(Debug, Clone, PartialEq)]
struct NameValueExpr {
    pub path: Ident,
    pub eq_token: Token![=],
    pub expr: Expr,
}

impl Parse for NameValueExpr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(NameValueExpr {
            path: input.parse()?,
            eq_token: input.parse()?,
            expr: input.parse()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct FieldThenParams {
    pub field: Field,
    pub comma: Option<Token![,]>,
    pub params: Punctuated<NameValueExpr, Token![,]>,
}

impl Parse for FieldThenParams {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let field = Field::parse_unnamed(input)?;
        let comma: Option<Token![,]> = input.parse()?;
        let params = if comma.is_some() {
            input.parse_terminated(NameValueExpr::parse)?
        } else {
            Punctuated::new()
        };

        Ok(FieldThenParams {
            field,
            comma,
            params,
        })
    }
}

fn gen_field(path: String, leaf: Field, out: &mut Map<String, Value>) -> Value {
    let leaf_type = leaf.ty;

    let leaf_attr = leaf
        .attrs
        .iter()
        .find(|attr| attr.path == syn::parse_quote!(rust_sitter::leaf));

    let leaf_params = leaf_attr.and_then(|a| {
        a.parse_args_with(Punctuated::<NameValueExpr, Token![,]>::parse_terminated)
            .ok()
    });

    let pattern_param = leaf_params.as_ref().and_then(|p| {
        p.iter()
            .find(|param| param.path == "pattern")
            .map(|p| p.expr.clone())
    });

    let text_param = leaf_params.as_ref().and_then(|p| {
        p.iter()
            .find(|param| param.path == "text")
            .map(|p| p.expr.clone())
    });

    let (inner_type, is_option) = if let Type::Path(p) = &leaf_type {
        let type_segment = p.path.segments.first().unwrap();
        if type_segment.ident == "Option" {
            let leaf_type = if let PathArguments::AngleBracketed(p) = &type_segment.arguments {
                if let GenericArgument::Type(t) = p.args.first().unwrap().clone() {
                    t
                } else {
                    panic!("Argument in angle brackets must be a type")
                }
            } else {
                panic!("Expected angle bracketed path");
            };

            (leaf_type, true)
        } else {
            (leaf_type.clone(), false)
        }
    } else {
        (leaf_type.clone(), false)
    };

    if let Some(Expr::Lit(lit)) = pattern_param {
        if let Lit::Str(s) = &lit.lit {
            out.insert(
                path.clone(),
                json!({
                    "type": "PATTERN",
                    "value": s.value(),
                }),
            );

            json!({
                "type": "SYMBOL",
                "name": path
            })
        } else {
            panic!("Expected string literal for pattern");
        }
    } else if let Some(Expr::Lit(lit)) = text_param {
        if let Lit::Str(s) = &lit.lit {
            out.insert(
                path.clone(),
                json!({
                    "type": "STRING",
                    "value": s.value(),
                }),
            );

            json!({
                "type": "SYMBOL",
                "name": path
            })
        } else {
            panic!("Expected string literal for text");
        }
    } else if let Type::Path(p) = &inner_type {
        let type_segment = p.path.segments.first().unwrap();
        if type_segment.ident == "Vec" {
            if is_option {
                panic!("Option<Vec> is not supported");
            }

            let leaf_type = if let PathArguments::AngleBracketed(p) = &type_segment.arguments {
                p.args.first().unwrap().clone()
            } else {
                panic!("Expected angle bracketed path");
            };

            let leaf_type = if let GenericArgument::Type(Type::Path(t)) = leaf_type {
                t
            } else {
                panic!("Expected type");
            };

            let delimited_attr = leaf
                .attrs
                .iter()
                .find(|attr| attr.path == syn::parse_quote!(rust_sitter::delimited));

            let delimited_params =
                delimited_attr.and_then(|a| a.parse_args_with(FieldThenParams::parse).ok());

            let delimiter_json = delimited_params
                .map(|p| gen_field(format!("{}_{}", path, "delimiter"), p.field, out));

            let repeat_attr = leaf
                .attrs
                .iter()
                .find(|attr| attr.path == syn::parse_quote!(rust_sitter::repeat));

            let repeat_params = repeat_attr.and_then(|a| {
                a.parse_args_with(Punctuated::<NameValueExpr, Token![,]>::parse_terminated)
                    .ok()
            });

            let repeat_non_empty = repeat_params
                .and_then(|p| {
                    p.iter()
                        .find(|param| param.path == "non_empty")
                        .map(|p| p.expr.clone())
                })
                .map(|e| e == syn::parse_quote!(true))
                .unwrap_or(false);

            let field_rule = json!({
                "type": "FIELD",
                "name": leaf.ident.unwrap().to_string(),
                "content": {
                    "type": "SYMBOL",
                    "name": leaf_type.path.segments.first().unwrap().ident.to_string(),
                }
            });

            if let Some(delimiter_json) = delimiter_json {
                let non_empty = json!({
                    "type": "SEQ",
                    "members": [
                        field_rule,
                        {
                            "type": "REPEAT",
                            "content": {
                                "type": "SEQ",
                                "members": [
                                    delimiter_json,
                                    field_rule,
                                ]
                            }
                        }
                    ]
                });

                if repeat_non_empty {
                    non_empty
                } else {
                    json!({
                        "type": "CHOICE",
                        "members": [
                            // optional
                            {
                                "type": "BLANK"
                            },
                            non_empty
                        ]
                    })
                }
            } else {
                json!({
                    "type": if repeat_non_empty {
                        "REPEAT1"
                    } else {
                        "REPEAT"
                    },
                    "content": field_rule
                })
            }
        } else {
            let (inner_type, _) = if type_segment.ident == "Box" {
                let inner_type = if let PathArguments::AngleBracketed(p) = &type_segment.arguments {
                    if let GenericArgument::Type(t) = p.args.first().unwrap().clone() {
                        t
                    } else {
                        panic!("Argument in angle brackets must be a type")
                    }
                } else {
                    panic!("Expected angle bracketed path");
                };

                (inner_type, true)
            } else if p.path.segments.len() == 1 {
                (inner_type.clone(), false)
            } else {
                panic!("Unexpected leaf type");
            };

            if let Type::Path(p) = &inner_type {
                if p.path.segments.len() == 1 {
                    json!({
                        "type": "SYMBOL",
                        "name": p.path.segments.first().unwrap().ident.to_string(),
                    })
                } else {
                    panic!("Unexpected leaf type");
                }
            } else {
                panic!("Unexpected leaf type");
            }
        }
    } else {
        panic!("Unexpected leaf type");
    }
}

fn gen_struct_or_variant(
    path: String,
    attrs: Vec<Attribute>,
    fields: Fields,
    out: &mut Map<String, Value>,
) {
    let children = fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let ident_str = field
                .ident
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or(format!("{}", i));

            let is_vec = if let Type::Path(p) = &field.ty {
                let type_segment = p.path.segments.first().unwrap();
                type_segment.ident == "Vec"
            } else {
                false
            };

            let is_option = if let Type::Path(p) = &field.ty {
                let type_segment = p.path.segments.first().unwrap();
                type_segment.ident == "Option"
            } else {
                false
            };

            let field_contents = gen_field(
                format!("{}_{}", path.clone(), ident_str),
                field.clone(),
                out,
            );

            if is_vec {
                field_contents
            } else {
                let core = json!({
                    "type": "FIELD",
                    "name": ident_str,
                    "content": field_contents
                });

                if is_option {
                    json!({
                        "type": "CHOICE",
                        "members": [
                            {
                                "type": "BLANK"
                            },
                            core
                        ]
                    })
                } else {
                    core
                }
            }
        })
        .collect::<Vec<Value>>();

    let prec_left_attr = attrs
        .iter()
        .find(|attr| attr.path == syn::parse_quote!(rust_sitter::prec_left));

    let prec_left_param = prec_left_attr.and_then(|a| a.parse_args_with(Expr::parse).ok());

    let seq_rule = json!({
        "type": "SEQ",
        "members": children
    });

    let rule = if let Some(Expr::Lit(lit)) = prec_left_param {
        if let Lit::Int(i) = &lit.lit {
            json!({
                "type": "PREC_LEFT",
                "value": i.base10_parse::<u32>().unwrap(),
                "content": seq_rule
            })
        } else {
            panic!("Expected integer literal for precedence");
        }
    } else {
        seq_rule
    };

    out.insert(path, rule);
}

fn generate_grammar(module: &ItemMod) -> Value {
    let mut rules_map = Map::new();
    // for some reason, source_file must be the first key for things to work
    rules_map.insert("source_file".to_string(), json!({}));

    let mut extras_list = vec![];

    let grammar_name = module
        .attrs
        .iter()
        .find_map(|a| {
            if a.path == syn::parse_quote!(rust_sitter::grammar) {
                let grammar_name_expr = a.parse_args_with(Expr::parse).ok();
                if let Some(Expr::Lit(ExprLit {
                    attrs: _,
                    lit: Lit::Str(s),
                })) = grammar_name_expr
                {
                    Some(s.value())
                } else {
                    panic!("Expected string literal for grammar name");
                }
            } else {
                None
            }
        })
        .expect("Each grammar must have a name");

    let (_, contents) = module.content.as_ref().unwrap();

    let root_type = contents
        .iter()
        .find_map(|item| match item {
            Item::Enum(ItemEnum { ident, attrs, .. })
            | Item::Struct(ItemStruct { ident, attrs, .. }) => {
                if attrs
                    .iter()
                    .any(|attr| attr.path == syn::parse_quote!(rust_sitter::language))
                {
                    Some(ident.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("Each parser must have the root type annotated with `#[rust_sitter::language]`")
        .to_string();

    contents.iter().for_each(|c| {
        let (symbol, attrs) = match c {
            Item::Enum(e) => {
                e.variants.iter().for_each(|v| {
                    gen_struct_or_variant(
                        format!("{}_{}", e.ident, v.ident),
                        v.attrs.clone(),
                        v.fields.clone(),
                        &mut rules_map,
                    )
                });

                let mut members: Vec<Value> = vec![];
                e.variants.iter().for_each(|v| {
                    let variant_path = format!("{}_{}", e.ident.clone(), v.ident);
                    members.push(json!({
                        "type": "SYMBOL",
                        "name": variant_path
                    }))
                });

                let rule = json!({
                    "type": "CHOICE",
                    "members": members
                });

                rules_map.insert(e.ident.to_string(), rule);

                (e.ident.to_string(), e.attrs.clone())
            }

            Item::Struct(s) => {
                gen_struct_or_variant(
                    s.ident.to_string(),
                    s.attrs.clone(),
                    s.fields.clone(),
                    &mut rules_map,
                );

                (s.ident.to_string(), s.attrs.clone())
            }

            _ => panic!(),
        };

        if attrs
            .iter()
            .any(|a| a.path == syn::parse_quote!(rust_sitter::extra))
        {
            extras_list.push(json!({
                "type": "SYMBOL",
                "name": symbol
            }));
        }
    });

    rules_map.insert(
        "source_file".to_string(),
        rules_map.get(&root_type).unwrap().clone(),
    );

    json!({
        "name": grammar_name,
        "rules": rules_map,
        "extras": extras_list
    })
}

fn generate_all_grammars(item: &Item, out: &mut Vec<String>) {
    if let Item::Mod(m) = item {
        m.content
            .iter()
            .for_each(|(_, items)| items.iter().for_each(|i| generate_all_grammars(i, out)));

        if m.attrs
            .iter()
            .any(|a| a.path == parse_quote!(rust_sitter::grammar))
        {
            out.push(generate_grammar(m).to_string())
        }
    }
}

pub fn generate_grammars(root_file: &Path) -> Vec<String> {
    let root_file = syn_inline_mod::parse_and_inline_modules(root_file).items;
    let mut out = vec![];
    root_file
        .iter()
        .for_each(|i| generate_all_grammars(i, &mut out));
    out
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::generate_grammar;

    #[test]
    fn enum_transformed_fields() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub enum Expression {
                    Number(
                        #[rust_sitter::leaf(pattern = r"\d+", transform = |v: &str| v.parse::<i32>().unwrap())]
                        i32
                    ),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn enum_recursive() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub enum Expression {
                    Number(
                        #[rust_sitter::leaf(pattern = r"\d+", transform = |v: &str| v.parse::<i32>().unwrap())]
                        i32
                    ),
                    Neg(
                        #[rust_sitter::leaf(text = "-", transform = |v| ())]
                        (),
                        Box<Expression>
                    ),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn enum_prec_left() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub enum Expression {
                    Number(
                        #[rust_sitter::leaf(pattern = r"\d+", transform = |v: &str| v.parse::<i32>().unwrap())]
                        i32
                    ),
                    #[rust_sitter::prec_left(1)]
                    Sub(
                        Box<Expression>,
                        #[rust_sitter::leaf(text = "-", transform = |v| ())]
                        (),
                        Box<Expression>
                    ),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn grammar_with_extras() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub enum Expression {
                    Number(
                        #[rust_sitter::leaf(pattern = r"\d+", transform = |v: &str| v.parse::<i32>().unwrap())]
                        i32
                    ),
                }

                #[rust_sitter::extra]
                struct Whitespace {
                    #[rust_sitter::leaf(pattern = r"\s", transform = |_v| ())]
                    _whitespace: (),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn grammar_unboxed_field() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub struct Language {
                    e: Expression,
                }

                pub enum Expression {
                    Number(
                        #[rust_sitter::leaf(pattern = r"\d+", transform = |v: &str| v.parse::<i32>().unwrap())]
                        i32
                    ),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn grammar_repeat() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            pub mod grammar {
                #[rust_sitter::language]
                pub struct NumberList {
                    #[rust_sitter::delimited(
                        #[rust_sitter::leaf(text = ",")]
                        ()
                    )]
                    numbers: Vec<Number>,
                }

                pub struct Number {
                    #[rust_sitter::leaf(pattern = r"\d+", transform = |v| v.parse().unwrap())]
                    v: i32,
                }

                #[rust_sitter::extra]
                struct Whitespace {
                    #[rust_sitter::leaf(pattern = r"\s")]
                    _whitespace: (),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn grammar_repeat1() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            pub mod grammar {
                #[rust_sitter::language]
                pub struct NumberList {
                    #[rust_sitter::repeat(non_empty = true)]
                    #[rust_sitter::delimited(
                        #[rust_sitter::leaf(text = ",")]
                        ()
                    )]
                    numbers: Vec<Number>,
                }

                pub struct Number {
                    #[rust_sitter::leaf(pattern = r"\d+", transform = |v| v.parse().unwrap())]
                    v: i32,
                }

                #[rust_sitter::extra]
                struct Whitespace {
                    #[rust_sitter::leaf(pattern = r"\s")]
                    _whitespace: (),
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }

    #[test]
    fn struct_optional() {
        let m = if let syn::Item::Mod(m) = parse_quote! {
            #[rust_sitter::grammar("test")]
            mod grammar {
                #[rust_sitter::language]
                pub struct Language {
                    #[rust_sitter::leaf(pattern = r"\d+", transform = |v| v.parse().unwrap())]
                    v: Option<i32>,
                    t: Option<Number>,
                }

                pub struct Number {
                    #[rust_sitter::leaf(pattern = r"\d+", transform = |v| v.parse().unwrap())]
                    v: i32
                }
            }
        } {
            m
        } else {
            panic!()
        };

        insta::assert_display_snapshot!(generate_grammar(&m));
    }
}
