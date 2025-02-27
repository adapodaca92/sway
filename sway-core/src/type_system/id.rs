use super::*;
use crate::engine_threading::*;

use fuel_abi_types::program_abi;

use std::fmt;

/// A identifier to uniquely refer to our type terms
#[derive(PartialEq, Eq, Hash, Clone, Copy, Ord, PartialOrd, Debug)]
pub struct TypeId(usize);

impl DisplayWithEngines for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>, engines: Engines<'_>) -> fmt::Result {
        write!(f, "{}", engines.help_out(engines.te().get(*self)))
    }
}

impl From<usize> for TypeId {
    fn from(o: usize) -> Self {
        TypeId(o)
    }
}

impl CollectTypesMetadata for TypeId {
    fn collect_types_metadata(
        &self,
        ctx: &mut CollectTypesMetadataContext,
    ) -> CompileResult<Vec<TypeMetadata>> {
        let mut warnings = vec![];
        let mut errors = vec![];
        let mut res = vec![];
        match ctx.type_engine.get(*self) {
            TypeInfo::UnknownGeneric {
                name,
                trait_constraints,
            } => {
                res.push(TypeMetadata::UnresolvedType(name, ctx.call_site_get(self)));
                for trait_constraint in trait_constraints.iter() {
                    res.extend(check!(
                        trait_constraint.collect_types_metadata(ctx),
                        continue,
                        warnings,
                        errors
                    ));
                }
            }
            TypeInfo::Placeholder(type_param) => {
                res.push(TypeMetadata::UnresolvedType(
                    type_param.name_ident,
                    ctx.call_site_get(self),
                ));
            }
            _ => {}
        }
        if let TypeInfo::UnknownGeneric {
            name,
            trait_constraints,
        } = ctx.type_engine.get(*self)
        {
            res.push(TypeMetadata::UnresolvedType(name, ctx.call_site_get(self)));
            for trait_constraint in trait_constraints.iter() {
                res.extend(check!(
                    trait_constraint.collect_types_metadata(ctx),
                    continue,
                    warnings,
                    errors
                ));
            }
        }
        if errors.is_empty() {
            ok(res, warnings, errors)
        } else {
            err(warnings, errors)
        }
    }
}

impl ReplaceSelfType for TypeId {
    fn replace_self_type(&mut self, engines: Engines<'_>, self_type: TypeId) {
        match engines.te().get(*self) {
            TypeInfo::SelfType => {
                *self = self_type;
            }
            TypeInfo::Enum {
                mut type_parameters,
                mut variant_types,
                ..
            } => {
                for type_parameter in type_parameters.iter_mut() {
                    type_parameter.replace_self_type(engines, self_type);
                }
                for variant_type in variant_types.iter_mut() {
                    variant_type.replace_self_type(engines, self_type);
                }
            }
            TypeInfo::Struct {
                mut type_parameters,
                mut fields,
                ..
            } => {
                for type_parameter in type_parameters.iter_mut() {
                    type_parameter.replace_self_type(engines, self_type);
                }
                for field in fields.iter_mut() {
                    field.replace_self_type(engines, self_type);
                }
            }
            TypeInfo::Tuple(mut type_arguments) => {
                for type_argument in type_arguments.iter_mut() {
                    type_argument.replace_self_type(engines, self_type);
                }
            }
            TypeInfo::Custom { type_arguments, .. } => {
                if let Some(mut type_arguments) = type_arguments {
                    for type_argument in type_arguments.iter_mut() {
                        type_argument.replace_self_type(engines, self_type);
                    }
                }
            }
            TypeInfo::Array(mut type_id, _) => {
                type_id.replace_self_type(engines, self_type);
            }
            TypeInfo::Storage { mut fields } => {
                for field in fields.iter_mut() {
                    field.replace_self_type(engines, self_type);
                }
            }
            TypeInfo::Unknown
            | TypeInfo::UnknownGeneric { .. }
            | TypeInfo::Str(_)
            | TypeInfo::UnsignedInteger(_)
            | TypeInfo::Boolean
            | TypeInfo::ContractCaller { .. }
            | TypeInfo::B256
            | TypeInfo::Numeric
            | TypeInfo::RawUntypedPtr
            | TypeInfo::RawUntypedSlice
            | TypeInfo::Contract
            | TypeInfo::ErrorRecovery
            | TypeInfo::Placeholder(_) => {}
        }
    }
}

impl SubstTypes for TypeId {
    fn subst_inner(&mut self, type_mapping: &TypeSubstMap, engines: Engines<'_>) {
        if let Some(matching_id) = type_mapping.find_match(*self, engines) {
            *self = matching_id;
        }
    }
}

impl UnconstrainedTypeParameters for TypeId {
    fn type_parameter_is_unconstrained(
        &self,
        engines: Engines<'_>,
        type_parameter: &TypeParameter,
    ) -> bool {
        let type_engine = engines.te();
        type_engine
            .get(*self)
            .type_parameter_is_unconstrained(engines, type_parameter)
    }
}

impl TypeId {
    pub(super) fn new(index: usize) -> TypeId {
        TypeId(index)
    }

    /// Returns the index that identifies the type.
    pub fn index(&self) -> usize {
        self.0
    }

    pub(crate) fn get_type_parameters(
        &self,
        type_engine: &TypeEngine,
    ) -> Option<Vec<TypeParameter>> {
        match type_engine.get(*self) {
            TypeInfo::Enum {
                type_parameters, ..
            } => (!type_parameters.is_empty()).then_some(type_parameters),
            TypeInfo::Struct {
                type_parameters, ..
            } => (!type_parameters.is_empty()).then_some(type_parameters),
            _ => None,
        }
    }

    /// Indicates of a given type is generic or not. Rely on whether the type is `Custom` and
    /// consider the special case where the resolved type is a struct or enum with a name that
    /// matches the name of the `Custom`.
    pub(crate) fn is_generic_parameter(
        self,
        type_engine: &TypeEngine,
        resolved_type_id: TypeId,
    ) -> bool {
        match (type_engine.get(self), type_engine.get(resolved_type_id)) {
            (
                TypeInfo::Custom { name, .. },
                TypeInfo::Enum {
                    name: enum_name, ..
                },
            ) => name != enum_name,
            (
                TypeInfo::Custom { name, .. },
                TypeInfo::Struct {
                    name: struct_name, ..
                },
            ) => name != struct_name,
            (TypeInfo::Custom { .. }, _) => true,
            _ => false,
        }
    }

    /// Return the components of a given (potentially generic) type while considering what it
    /// actually resolves to. These components are essentially of type of
    /// `program_abi::TypeApplication`.  The method below also updates the provided list of
    /// `program_abi::TypeDeclaration`s  to add the newly discovered types.
    pub(crate) fn get_json_type_components(
        &self,
        type_engine: &TypeEngine,
        types: &mut Vec<program_abi::TypeDeclaration>,
        resolved_type_id: TypeId,
    ) -> Option<Vec<program_abi::TypeApplication>> {
        match type_engine.get(*self) {
            TypeInfo::Enum { variant_types, .. } => {
                // A list of all `program_abi::TypeDeclaration`s needed for the enum variants
                let variants = variant_types
                    .iter()
                    .map(|x| program_abi::TypeDeclaration {
                        type_id: x.initial_type_id.index(),
                        type_field: x.initial_type_id.get_json_type_str(type_engine, x.type_id),
                        components: x.initial_type_id.get_json_type_components(
                            type_engine,
                            types,
                            x.type_id,
                        ),
                        type_parameters: x.initial_type_id.get_json_type_parameters(
                            type_engine,
                            types,
                            x.type_id,
                        ),
                    })
                    .collect::<Vec<_>>();
                types.extend(variants);

                // Generate the JSON data for the enum. This is basically a list of
                // `program_abi::TypeApplication`s
                Some(
                    variant_types
                        .iter()
                        .map(|x| program_abi::TypeApplication {
                            name: x.name.to_string(),
                            type_id: x.initial_type_id.index(),
                            type_arguments: x.initial_type_id.get_json_type_arguments(
                                type_engine,
                                types,
                                x.type_id,
                            ),
                        })
                        .collect(),
                )
            }
            TypeInfo::Struct { fields, .. } => {
                // A list of all `program_abi::TypeDeclaration`s needed for the struct fields
                let field_types = fields
                    .iter()
                    .map(|x| program_abi::TypeDeclaration {
                        type_id: x.initial_type_id.index(),
                        type_field: x.initial_type_id.get_json_type_str(type_engine, x.type_id),
                        components: x.initial_type_id.get_json_type_components(
                            type_engine,
                            types,
                            x.type_id,
                        ),
                        type_parameters: x.initial_type_id.get_json_type_parameters(
                            type_engine,
                            types,
                            x.type_id,
                        ),
                    })
                    .collect::<Vec<_>>();
                types.extend(field_types);

                // Generate the JSON data for the struct. This is basically a list of
                // `program_abi::TypeApplication`s
                Some(
                    fields
                        .iter()
                        .map(|x| program_abi::TypeApplication {
                            name: x.name.to_string(),
                            type_id: x.initial_type_id.index(),
                            type_arguments: x.initial_type_id.get_json_type_arguments(
                                type_engine,
                                types,
                                x.type_id,
                            ),
                        })
                        .collect(),
                )
            }
            TypeInfo::Array(..) => {
                if let TypeInfo::Array(elem_ty, _) = type_engine.get(resolved_type_id) {
                    // The `program_abi::TypeDeclaration`s needed for the array element type
                    let elem_json_ty = program_abi::TypeDeclaration {
                        type_id: elem_ty.initial_type_id.index(),
                        type_field: elem_ty
                            .initial_type_id
                            .get_json_type_str(type_engine, elem_ty.type_id),
                        components: elem_ty.initial_type_id.get_json_type_components(
                            type_engine,
                            types,
                            elem_ty.type_id,
                        ),
                        type_parameters: elem_ty.initial_type_id.get_json_type_parameters(
                            type_engine,
                            types,
                            elem_ty.type_id,
                        ),
                    };
                    types.push(elem_json_ty);

                    // Generate the JSON data for the array. This is basically a single
                    // `program_abi::TypeApplication` for the array element type
                    Some(vec![program_abi::TypeApplication {
                        name: "__array_element".to_string(),
                        type_id: elem_ty.initial_type_id.index(),
                        type_arguments: elem_ty.initial_type_id.get_json_type_arguments(
                            type_engine,
                            types,
                            elem_ty.type_id,
                        ),
                    }])
                } else {
                    unreachable!();
                }
            }
            TypeInfo::Tuple(_) => {
                if let TypeInfo::Tuple(fields) = type_engine.get(resolved_type_id) {
                    // A list of all `program_abi::TypeDeclaration`s needed for the tuple fields
                    let fields_types = fields
                        .iter()
                        .map(|x| program_abi::TypeDeclaration {
                            type_id: x.initial_type_id.index(),
                            type_field: x.initial_type_id.get_json_type_str(type_engine, x.type_id),
                            components: x.initial_type_id.get_json_type_components(
                                type_engine,
                                types,
                                x.type_id,
                            ),
                            type_parameters: x.initial_type_id.get_json_type_parameters(
                                type_engine,
                                types,
                                x.type_id,
                            ),
                        })
                        .collect::<Vec<_>>();
                    types.extend(fields_types);

                    // Generate the JSON data for the tuple. This is basically a list of
                    // `program_abi::TypeApplication`s
                    Some(
                        fields
                            .iter()
                            .map(|x| program_abi::TypeApplication {
                                name: "__tuple_element".to_string(),
                                type_id: x.initial_type_id.index(),
                                type_arguments: x.initial_type_id.get_json_type_arguments(
                                    type_engine,
                                    types,
                                    x.type_id,
                                ),
                            })
                            .collect(),
                    )
                } else {
                    unreachable!()
                }
            }
            TypeInfo::Custom { type_arguments, .. } => {
                if !self.is_generic_parameter(type_engine, resolved_type_id) {
                    // A list of all `program_abi::TypeDeclaration`s needed for the type arguments
                    let type_args = type_arguments
                        .unwrap_or_default()
                        .iter()
                        .zip(
                            resolved_type_id
                                .get_type_parameters(type_engine)
                                .unwrap_or_default()
                                .iter(),
                        )
                        .map(|(v, p)| program_abi::TypeDeclaration {
                            type_id: v.initial_type_id.index(),
                            type_field: v.initial_type_id.get_json_type_str(type_engine, p.type_id),
                            components: v.initial_type_id.get_json_type_components(
                                type_engine,
                                types,
                                p.type_id,
                            ),
                            type_parameters: v.initial_type_id.get_json_type_parameters(
                                type_engine,
                                types,
                                p.type_id,
                            ),
                        })
                        .collect::<Vec<_>>();
                    types.extend(type_args);

                    resolved_type_id.get_json_type_components(type_engine, types, resolved_type_id)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Return the type parameters of a given (potentially generic) type while considering what it
    /// actually resolves to. These parameters are essentially of type of `usize` which are
    /// basically the IDs of some set of `program_abi::TypeDeclaration`s. The method below also
    /// updates the provide list of `program_abi::TypeDeclaration`s  to add the newly discovered
    /// types.
    pub(crate) fn get_json_type_parameters(
        &self,
        type_engine: &TypeEngine,
        types: &mut Vec<program_abi::TypeDeclaration>,
        resolved_type_id: TypeId,
    ) -> Option<Vec<usize>> {
        match self.is_generic_parameter(type_engine, resolved_type_id) {
            true => None,
            false => resolved_type_id.get_type_parameters(type_engine).map(|v| {
                v.iter()
                    .map(|v| v.get_json_type_parameter(type_engine, types))
                    .collect::<Vec<_>>()
            }),
        }
    }

    /// Return the type arguments of a given (potentially generic) type while considering what it
    /// actually resolves to. These arguments are essentially of type of
    /// `program_abi::TypeApplication`. The method below also updates the provided list of
    /// `program_abi::TypeDeclaration`s  to add the newly discovered types.
    pub(crate) fn get_json_type_arguments(
        &self,
        type_engine: &TypeEngine,
        types: &mut Vec<program_abi::TypeDeclaration>,
        resolved_type_id: TypeId,
    ) -> Option<Vec<program_abi::TypeApplication>> {
        let resolved_params = resolved_type_id.get_type_parameters(type_engine);
        match type_engine.get(*self) {
            TypeInfo::Custom {
                type_arguments: Some(type_arguments),
                ..
            } => (!type_arguments.is_empty()).then_some({
                let resolved_params = resolved_params.unwrap_or_default();
                let json_type_arguments = type_arguments
                    .iter()
                    .zip(resolved_params.iter())
                    .map(|(v, p)| program_abi::TypeDeclaration {
                        type_id: v.initial_type_id.index(),
                        type_field: v.initial_type_id.get_json_type_str(type_engine, p.type_id),
                        components: v.initial_type_id.get_json_type_components(
                            type_engine,
                            types,
                            p.type_id,
                        ),
                        type_parameters: v.initial_type_id.get_json_type_parameters(
                            type_engine,
                            types,
                            p.type_id,
                        ),
                    })
                    .collect::<Vec<_>>();
                types.extend(json_type_arguments);

                type_arguments
                    .iter()
                    .map(|arg| program_abi::TypeApplication {
                        name: "".to_string(),
                        type_id: arg.initial_type_id.index(),
                        type_arguments: arg.initial_type_id.get_json_type_arguments(
                            type_engine,
                            types,
                            arg.type_id,
                        ),
                    })
                    .collect::<Vec<_>>()
            }),
            TypeInfo::Enum {
                type_parameters, ..
            }
            | TypeInfo::Struct {
                type_parameters, ..
            } => {
                // Here, type_id for each type parameter should contain resolved types
                let json_type_arguments = type_parameters
                    .iter()
                    .map(|v| program_abi::TypeDeclaration {
                        type_id: v.type_id.index(),
                        type_field: v.type_id.get_json_type_str(type_engine, v.type_id),
                        components: v.type_id.get_json_type_components(
                            type_engine,
                            types,
                            v.type_id,
                        ),
                        type_parameters: v.type_id.get_json_type_parameters(
                            type_engine,
                            types,
                            v.type_id,
                        ),
                    })
                    .collect::<Vec<_>>();
                types.extend(json_type_arguments);

                Some(
                    type_parameters
                        .iter()
                        .map(|arg| program_abi::TypeApplication {
                            name: "".to_string(),
                            type_id: arg.type_id.index(),
                            type_arguments: arg.type_id.get_json_type_arguments(
                                type_engine,
                                types,
                                arg.type_id,
                            ),
                        })
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        }
    }

    pub fn json_abi_str(&self, type_engine: &TypeEngine) -> String {
        type_engine.get(*self).json_abi_str(type_engine)
    }

    /// Gives back a string that represents the type, considering what it resolves to
    pub(crate) fn get_json_type_str(
        &self,
        type_engine: &TypeEngine,
        resolved_type_id: TypeId,
    ) -> String {
        if self.is_generic_parameter(type_engine, resolved_type_id) {
            format!(
                "generic {}",
                type_engine.get(*self).json_abi_str(type_engine)
            )
        } else {
            match (type_engine.get(*self), type_engine.get(resolved_type_id)) {
                (TypeInfo::Custom { .. }, TypeInfo::Struct { .. }) => {
                    format!(
                        "struct {}",
                        type_engine.get(*self).json_abi_str(type_engine)
                    )
                }
                (TypeInfo::Custom { .. }, TypeInfo::Enum { .. }) => {
                    format!("enum {}", type_engine.get(*self).json_abi_str(type_engine))
                }
                (TypeInfo::Tuple(fields), TypeInfo::Tuple(resolved_fields)) => {
                    assert_eq!(fields.len(), resolved_fields.len());
                    let field_strs = fields
                        .iter()
                        .map(|_| "_".to_string())
                        .collect::<Vec<String>>();
                    format!("({})", field_strs.join(", "))
                }
                (TypeInfo::Array(_, count), TypeInfo::Array(_, resolved_count)) => {
                    assert_eq!(count.val(), resolved_count.val());
                    format!("[_; {}]", count.val())
                }
                (TypeInfo::Custom { .. }, _) => {
                    format!(
                        "generic {}",
                        type_engine.get(*self).json_abi_str(type_engine)
                    )
                }
                _ => type_engine.get(*self).json_abi_str(type_engine),
            }
        }
    }
}
