//! sid's asset source: its own brand SVGs, then the full Lucide catalog.
//!
//! The brand marks under `assets/icons/` are the single-path monochrome SVGs from
//! Simple Icons (<https://simpleicons.org>, `simple-icons` npm package), released under
//! CC0 1.0. The marks themselves are trademarks of Docker, Inc. and The Linux Foundation
//! (Kubernetes) and are used here only to identify those products. They carry no fill,
//! so they take the ambient text colour like every Lucide icon.
//!
//! Every place that installs assets (`main.rs`, the gallery, the test harness) must use
//! [`SidAssets`], or a brand icon renders as nothing.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Asset path of the Docker mark.
pub const DOCKER: &str = "icons/brand/docker.svg";
/// Asset path of the Kubernetes mark.
pub const KUBERNETES: &str = "icons/brand/kubernetes.svg";

const BRAND: &[(&str, &[u8])] = &[
    (DOCKER, include_bytes!("../assets/icons/docker.svg")),
    (KUBERNETES, include_bytes!("../assets/icons/kubernetes.svg")),
];

/// Brand SVGs first, everything else from `gpui_kit_assets::AllAssets`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SidAssets;

impl AssetSource for SidAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match BRAND.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit_assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out = gpui_kit_assets::AllAssets.list(path)?;
        out.extend(
            BRAND
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(p, _)| SharedString::from(*p)),
        );
        Ok(out)
    }
}
