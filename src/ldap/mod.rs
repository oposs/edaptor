//! LDAP layer: TLS settings and the background worker thread (the only code that
//! touches the network).

pub mod ldif;
pub mod result;
pub mod tls;
pub mod worker;
