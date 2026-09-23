//! Server-rendered operator console.
//!
//! Every page and fragment is a pure function from the JSON values the store
//! and run directory already produce to an HTML string. The `/api/*` routes
//! serve those same values verbatim, so the HTML and JSON views cannot
//! disagree, and every render function is testable without a socket. Vendored
//! htmx drives fragment swaps; the local theme script remembers appearance.

pub mod agents;
pub mod app;
pub mod budget;
pub mod edit;
pub mod forecast;
pub mod form;
pub mod html;
pub mod policy;

/// How many URLs the content projection asks for, named in one place so the
/// query and any sentence about it read the same number.
pub const CONTENT_CAP: usize = 50;

/// How many internal corpus paths the internal-use projection asks for.
pub const INTERNAL_CAP: usize = 50;
