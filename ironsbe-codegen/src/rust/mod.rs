//! Rust code generation modules.

pub mod enums;
pub(crate) mod fields;
pub mod groups;
pub mod messages;
pub(crate) mod names;
pub(crate) mod readers;
pub mod types;
pub(crate) mod var_data;
pub(crate) mod var_parts;

pub use enums::EnumGenerator;
pub use groups::GroupGenerator;
pub use messages::MessageGenerator;
pub use types::TypeGenerator;
