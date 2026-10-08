//! Resolves vite_rails' tag helpers at build time from the Vite manifest,
//! as the importmap is resolved from `config/importmap.rb`.
//!
//! In production vite_rails reads `public/vite/.vite/manifest.json` (from
//! `bin/vite build`) on each render. A binary contains one build, so the
//! tags can be computed at build time. `vite_javascript_tag "inertia.jsx"` becomes
//! the HTML vite_rails renders for it — the entry's `<script>`, a
//! `modulepreload` link per imported chunk, a stylesheet link per CSS
//! file — as a safe String literal in the view. `vite_client_tag` and
//! `vite_react_refresh_tag` render nothing without the dev server, and
//! `inertia_ssr_head` nothing without SSR, so they become `""`.
//!
//! A tag helper call in an app with no manifest, or one that names an
//! entry missing from the manifest, stays as written, and the survey
//! reports it, because the page would load without its scripts.

use std::path::Path;

use crate::App;
use crate::app::{ViteChunk, ViteManifest};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::vfs::Vfs;

use super::{IngestError, survey};

const EMPTY_HELPERS: &[&str] = &["vite_client_tag", "vite_react_refresh_tag", "inertia_ssr_head"];

pub fn read_manifest<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<ViteManifest> {
    let public_dir = public_output_dir(vfs, dir);
    let base = dir.join("public").join(&public_dir);
    let path = [base.join(".vite/manifest.json"), base.join("manifest.json")]
        .into_iter()
        .find(|p| vfs.exists(p))?;
    let bytes = vfs.read(&path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mut chunks = std::collections::BTreeMap::new();
    for (name, entry) in json.as_object()? {
        let strings = |key: &str| -> Vec<String> {
            entry
                .get(key)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        let Some(file) = entry.get("file").and_then(|f| f.as_str()) else { continue };
        chunks.insert(
            name.clone(),
            ViteChunk { file: file.to_string(), css: strings("css"), imports: strings("imports") },
        );
    }
    Some(ViteManifest { public_dir, digest: fnv1a64_hex(&bytes), chunks })
}

/// `publicOutputDir` of the production (else `all`) section of
/// `config/vite.json`; vite_ruby's default is `vite`.
fn public_output_dir<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> String {
    let config = vfs
        .read(&dir.join("config/vite.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    ["production", "all"]
        .iter()
        .find_map(|section| {
            config.as_ref()?.get(section)?.get("publicOutputDir")?.as_str().map(str::to_string)
        })
        .unwrap_or_else(|| "vite".to_string())
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn lower_vite_tags(app: &mut App) {
    let manifest = app.vite_manifest.clone();
    for view in &mut app.views {
        rewrite(&mut view.body, manifest.as_ref());
    }
}

fn rewrite(e: &mut Expr, manifest: Option<&ViteManifest>) {
    if let Some(html) = tag_html(e, manifest) {
        let span = e.span;
        let lit = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: html } });
        *e = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(lit),
                method: Symbol::from("html_safe"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, manifest));
}

fn tag_html(e: &Expr, manifest: Option<&ViteManifest>) -> Option<String> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
    let method = method.as_str();
    if EMPTY_HELPERS.contains(&method) && args.is_empty() {
        return Some(String::new());
    }
    let ext = match method {
        "vite_javascript_tag" => "js",
        "vite_typescript_tag" => "ts",
        _ => return None,
    };
    let gap = |why: String| {
        let file = super::sources::path_of(e.span.file).unwrap_or_default();
        survey::record(&IngestError::Unsupported { file, message: format!("`{method}`: {why}") });
        None
    };
    let Some(manifest) = manifest else {
        return gap("no Vite build manifest (run `bin/vite build` before roundhouse)".to_string());
    };
    let mut names = Vec::new();
    for a in args {
        match &*a.node {
            ExprNode::Lit { value: Literal::Str { value } } => names.push(value.clone()),
            ExprNode::Lit { value: Literal::Sym { value } } => names.push(value.as_str().to_string()),
            _ => return gap("options or a non-literal entry name are not modeled".to_string()),
        }
    }
    let mut entries = Vec::new();
    for name in &names {
        let key = entry_key(name, ext);
        let Some(chunk) = manifest.chunks.get(&key) else {
            return gap(format!("entry `{key}` is not in the Vite manifest"));
        };
        entries.push(chunk);
    }
    Some(tags(manifest, &entries))
}

/// vite_ruby's `resolve_entry_name`: an extension for the entry type when
/// the name has none, and the entrypoints directory when it is not nested.
fn entry_key(name: &str, ext: &str) -> String {
    let name = if Path::new(name).extension().is_none() { format!("{name}.{ext}") } else { name.to_string() };
    if let Some(rest) = name.strip_prefix("~/") {
        return rest.to_string();
    }
    if name.contains('/') { name } else { format!("entrypoints/{name}") }
}

/// vite_rails' `vite_javascript_tag` output: the scripts, the imported
/// chunks depth-first (`import_chunks_for`), then every entry's and
/// chunk's CSS.
fn tags(manifest: &ViteManifest, entries: &[&ViteChunk]) -> String {
    let asset = |file: &str| format!("/{}/{}", manifest.public_dir, file);
    let mut seen = std::collections::HashSet::new();
    let mut imports: Vec<&ViteChunk> = Vec::new();
    for entry in entries {
        import_chunks(manifest, entry, &mut seen, &mut imports);
    }
    let mut html: String = entries
        .iter()
        .map(|e| format!("<script src=\"{}\" crossorigin=\"\" type=\"module\"></script>", asset(&e.file)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut preload: Vec<String> = Vec::new();
    for chunk in &imports {
        let href = asset(&chunk.file);
        if !preload.contains(&href) {
            preload.push(href);
        }
    }
    html.push_str(
        &preload
            .iter()
            .map(|href| format!("<link rel=\"modulepreload\" href=\"{href}\" as=\"script\" crossorigin=\"\">"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut css: Vec<String> = Vec::new();
    for chunk in entries.iter().copied().chain(imports.iter().copied()) {
        for c in &chunk.css {
            let href = asset(c);
            if !css.contains(&href) {
                css.push(href);
            }
        }
    }
    html.push_str(
        &css.iter()
            .map(|href| format!("<link rel=\"stylesheet\" crossorigin=\"\" href=\"{href}\" />"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    html
}

fn import_chunks<'a>(
    manifest: &'a ViteManifest,
    chunk: &ViteChunk,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<&'a ViteChunk>,
) {
    for name in &chunk.imports {
        let Some(import) = manifest.chunks.get(name) else { continue };
        if !seen.insert(import.file.clone()) {
            continue;
        }
        import_chunks(manifest, import, seen, out);
        out.push(import);
    }
}
