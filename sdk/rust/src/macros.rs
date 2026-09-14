macro_rules! string_enum {
    ($(#[$meta:meta])* pub enum $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $(#[doc = concat!("Wire value `", $wire, "`.")]
            #[serde(rename = $wire)]
            $variant),+
        }
        impl $name {
            /// The exact public wire value.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}

macro_rules! event_enum {
    ($tag:literal; $(#[$meta:meta])* pub enum $name:ident { $($variant:ident => $wire:literal { $($field:ident: $ty:ty),* $(,)? }),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
        #[serde(tag = $tag)]
        pub enum $name {
            $(#[doc = concat!("Durable `", $wire, "` record.")]
            #[serde(rename = $wire)]
            $variant { $(#[doc = concat!("The `", stringify!($field), "` field from this record.")]
                $field: $ty),* }),+
        }
    };
}
