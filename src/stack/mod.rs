//! Generation and lifecycle of the global Traefik stack and the host-side
//! dnsmasq drop-in.

pub mod certs;
pub mod dnsmasq;
pub mod lifecycle;
pub mod paths;
pub mod resolver;
pub mod templates;
