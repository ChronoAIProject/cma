//! Schema-only views for Serde's internally tagged newtype variants.
use std::{borrow::Cow, marker::PhantomData};
use utoipa::{
    openapi::{schema::Object, schema::Schema, schema::Type, RefOr},
    PartialSchema, ToSchema,
};

/// Inline this view as a variant's `value_type` when its enum uses `tag = "type"`.
///
/// The derive combines a newtype payload and its discriminator with `allOf`.
/// A closed payload must therefore declare the discriminator too. This view
/// preserves the payload's fields, required list and unknown-field rejection;
/// the enclosing enum still supplies the required, exact discriminator value.
/// The standalone payload schema and all runtime Serde behavior stay unchanged.
pub struct TypeTagged<T>(PhantomData<T>);

// Concrete names let the derive inline our public PartialSchema implementation
// without requiring its private generic-composition trait.
pub type NativeLarkSource = TypeTagged<crate::channel::NativeLarkSource>;
pub type NativeLarkIdentity = TypeTagged<crate::channel::NativeLarkIdentity>;
pub type InteractionResult = TypeTagged<crate::activity::InteractionResult>;

impl<T: ToSchema> PartialSchema for TypeTagged<T> {
    fn schema() -> RefOr<Schema> {
        let RefOr::T(Schema::Object(mut object)) = T::schema() else {
            panic!("internally tagged payload must have an object schema");
        };
        assert!(!object.properties.contains_key("type"));
        object.properties.insert(
            "type".into(),
            Object::builder().schema_type(Type::String).into(),
        );
        object.into()
    }
}

impl<T: ToSchema> ToSchema for TypeTagged<T> {
    fn name() -> Cow<'static, str> {
        Cow::Owned(format!("TypeTagged_{}", T::name()))
    }

    fn schemas(schemas: &mut Vec<(String, RefOr<Schema>)>) {
        // Retain the standalone public component as well as its dependencies.
        schemas.push((T::name().into_owned(), T::schema()));
        T::schemas(schemas);
    }
}
