//! Scene model and file format loaders, shared by the Poly Loupe app and its Explorer thumbnail
//! handler (which renders without a GPU, inside Windows' thumbnail process).

pub mod color;
pub mod formats;
pub mod i18n;
pub mod loader;
pub mod scene;
pub mod thumbnail;
