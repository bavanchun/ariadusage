//! Cookie parsing, source resolution, domain matching, and profile merging.

pub mod domains;
pub mod merge;
pub mod normalize;
pub mod resolve;

pub use domains::{DeclaredDomains, domain_matches};
pub use merge::{CookieRecord, CookieStoreKind, CookieStoreRecords, MergedProfile, merge_profiles};
pub use normalize::{filtered_header, normalize, pairs};
pub use resolve::{CookieResolution, ImportAuthorized, resolve_cookie_source, token_to_header};
