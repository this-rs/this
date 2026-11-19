//! # The drawing of [`Brick::Image`]: an `<img>` fed by a data URL, never a network request.
//!
//! Two sources, one rule — nothing leaves the view:
//! - a module file (`source.vfs`) : the host reads the bytes, this module makes a Blob URL
//!   and revokes it on unmount (the same discipline as the `html` node) ;
//! - a row's data URL (`source.entity` + `source.field`) : the field must hold a
//!   `data:image/…` URL. A remote URL (`http(s)://`, `//`) is refused, never fetched.
//!
//! The `<img>` is lazy and async-decoded, carries its `alt`, and — when the load fails —
//! shows a visible "image unavailable" tile instead of a hole. The pure decisions (height
//! clamp, fit, content type, the refusal of a remote URL) are testable without a browser.

use leptos::prelude::*;
use this_descriptor::views::{Brick, IMAGE_HEIGHT_MAX, IMAGE_HEIGHT_MIN, ImageFit, image_resolve};

use crate::views::data;
use crate::views::render::{self, Ctx};
use crate::views::style::class;

/// The host's reader of a module file, and nothing else: the same async VFS read the
/// `html` node uses.
pub type ReadFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>>>>;

/// What the host provides to draw a VFS image: read the bytes of a module file.
#[derive(Clone, Copy)]
pub struct ImageHost {
    pub read: Callback<String, ReadFuture>,
}

/// The height actually used: the declared one clamped into `24..=600`; `None` when absent
/// (the intrinsic height then decides).
pub fn clamped_height(declared: Option<u32>) -> Option<u32> {
    declared.map(|h| h.clamp(IMAGE_HEIGHT_MIN, IMAGE_HEIGHT_MAX))
}

/// The `object-fit` value: `cover` (default) or `contain`.
pub fn object_fit(fit: Option<ImageFit>) -> &'static str {
    match fit {
        Some(ImageFit::Contain) => "contain",
        _ => "cover",
    }
}

/// The MIME type of an image file, from its extension. An unknown extension yields
/// `application/octet-stream`, which the `<img>` fails to decode and shows the unavailable
/// tile — a bad file must not pass for a loaded image.
pub fn content_type_of(path: &str) -> &'static str {
    let bas = path.to_ascii_lowercase();
    if bas.ends_with(".png") {
        "image/png"
    } else if bas.ends_with(".jpg") || bas.ends_with(".jpeg") {
        "image/jpeg"
    } else if bas.ends_with(".gif") {
        "image/gif"
    } else if bas.ends_with(".webp") {
        "image/webp"
    } else if bas.ends_with(".avif") {
        "image/avif"
    } else if bas.ends_with(".svg") {
        "image/svg+xml"
    } else if bas.ends_with(".bmp") {
        "image/bmp"
    } else {
        "application/octet-stream"
    }
}

/// The source a row's field may carry, or the reason it is refused.
///
/// Only a `data:image/…` URL is drawn. A remote URL (`http(s)://`, `//`) is the one thing a
/// view must never request — it is refused by name. Anything else (plain text, a relative
/// path) is not an image either.
pub fn src_of_value(value: &str) -> Result<String, &'static str> {
    let trimmed = value.trim();
    if trimmed.starts_with("data:image/") {
        Ok(trimmed.to_owned())
    } else if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("//")
    {
        Err("image_remote_refused")
    } else {
        Err("image_invalid_source")
    }
}

/// The fallback tile: a broken image must be visible, not a hole.
fn unavailable(alt: &str) -> AnyView {
    let alt = alt.to_owned();
    view! {
        <div class=class::IMAGE_UNAVAILABLE role="img" aria-label=alt>
            "image indisponible"
        </div>
    }
    .into_any()
}

/// The `<img>` shared by both sources. `errored` flips to the unavailable tile when the
/// browser cannot decode the bytes.
fn picture(src: String, alt: String, height: Option<u32>, fit: Option<ImageFit>) -> AnyView {
    let errored = RwSignal::new(false);
    let mut style = format!("object-fit:{};max-width:100%", object_fit(fit));
    if let Some(h) = clamped_height(height) {
        style.push_str(&format!(";height:{h}px"));
    }
    let alt_for_error = alt.clone();
    view! {
        {move || {
            if errored.get() {
                unavailable(&alt_for_error)
            } else {
                view! {
                    <img
                        class=class::IMAGE
                        src=src.clone()
                        alt=alt.clone()
                        loading="lazy"
                        decoding="async"
                        data-image-alt=alt.clone()
                        style=style.clone()
                        on:error=move |_| errored.set(true)
                    />
                }
                .into_any()
            }
        }}
    }
    .into_any()
}

/// The whole brick: resolve the source, then draw.
pub fn render_image(ctx: Ctx, brick: &Brick) -> AnyView {
    let Brick::Image {
        source,
        alt,
        height,
        fit,
    } = brick
    else {
        return render::fallback_public("image");
    };
    let alt = alt.clone();
    let height = *height;
    let fit = *fit;
    if let Some(path) = source.vfs.as_deref() {
        render_vfs(ctx, path, alt, height, fit)
    } else if let (Some(entity), Some(field)) = (source.entity.as_deref(), source.field.as_deref())
    {
        render_field(ctx, entity, field, alt, height, fit)
    } else {
        render::fallback_public("image : source absente")
    }
}

/// A module file: resolve, read, Blob URL, revoke on unmount.
fn render_vfs(
    ctx: Ctx,
    path: &str,
    alt: String,
    height: Option<u32>,
    fit: Option<ImageFit>,
) -> AnyView {
    let full = ctx
        .module
        .try_with_value(|m| image_resolve(&m.name, &m.version, path))
        .unwrap_or_else(|| Err(("image_vfs_outside_module", "module indisponible".into())));
    let full = match full {
        Ok(full) => full,
        Err((reason, why)) => return render::fallback_public(&format!("image {reason} : {why}")),
    };
    let Some(host) = use_context::<ImageHost>() else {
        return render::fallback_public("image : l'hôte ne lit pas les fichiers du module");
    };
    mount_vfs(host, full, alt, height, fit)
}

#[cfg(not(target_arch = "wasm32"))]
fn mount_vfs(
    _host: ImageHost,
    full: String,
    _alt: String,
    _height: Option<u32>,
    _fit: Option<ImageFit>,
) -> AnyView {
    render::fallback_public(&format!(
        "image : {full} ne se monte que dans le navigateur"
    ))
}

#[cfg(target_arch = "wasm32")]
fn mount_vfs(
    host: ImageHost,
    full: String,
    alt: String,
    height: Option<u32>,
    fit: Option<ImageFit>,
) -> AnyView {
    let state = RwSignal::new(None::<Result<String, String>>);
    let created = StoredValue::new_local(None::<String>);
    {
        let full = full.clone();
        let read = host.read.run(full.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let verdict = match read.await {
                None => Err(format!("fichier introuvable ({full})")),
                Some(bytes) => {
                    let parts = js_sys::Array::new();
                    parts.push(&js_sys::Uint8Array::from(bytes.as_slice()));
                    let opts = web_sys::BlobPropertyBag::new();
                    opts.set_type(content_type_of(&full));
                    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
                        .and_then(|b| web_sys::Url::create_object_url_with_blob(&b))
                        .map_err(|_| "impossible de créer le Blob".to_owned())
                }
            };
            if let Ok(url) = &verdict {
                match state.try_set(Some(verdict.clone())) {
                    Some(_) => {
                        let _ = web_sys::Url::revoke_object_url(url);
                    }
                    None => created.set_value(Some(url.clone())),
                }
            } else {
                let _ = state.try_set(Some(verdict));
            }
        });
    }
    on_cleanup(move || {
        if let Some(Some(url)) = created.try_get_value() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });
    view! {
        {move || match state.get() {
            None => view! { <div class=class::LOADING role="status">"Chargement…"</div> }.into_any(),
            Some(Err(why)) => render::fallback_public(&format!("image : {why}")),
            Some(Ok(url)) => picture(url, alt.clone(), height, fit),
        }}
    }
    .into_any()
}

/// A row's data URL: read the field reactively, and draw only a `data:image/…` value.
fn render_field(
    ctx: Ctx,
    entity: &str,
    field: &str,
    alt: String,
    height: Option<u32>,
    fit: Option<ImageFit>,
) -> AnyView {
    let entity = entity.to_owned();
    let field = field.to_owned();
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, None, &[], 0, None) {
            data::Reading::Rows(rows) => {
                let value = rows
                    .rows
                    .first()
                    .copied()
                    .and_then(|row| data::value_of(row, &field))
                    .and_then(|v| v.as_str());
                match value.and_then(|v| src_of_value(v).ok()) {
                    Some(src) => picture(src, alt.clone(), height, fit),
                    None => {
                        if let Some(remote) = value.and_then(|v| {
                            (v.starts_with("http://")
                                || v.starts_with("https://")
                                || v.starts_with("//"))
                            .then(|| v.to_owned())
                        }) {
                            render::log_refusal(&format!(
                                "image : image_remote_refused — `{remote}` est une URL distante"
                            ));
                        }
                        unavailable(&alt)
                    }
                }
            }
            other => render::tile_for(ctx, &entity, &other, ""),
        }
    })
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_is_clamped_and_absent_means_intrinsic() {
        assert_eq!(clamped_height(None), None);
        assert_eq!(clamped_height(Some(10)), Some(24));
        assert_eq!(clamped_height(Some(9999)), Some(600));
        assert_eq!(clamped_height(Some(160)), Some(160));
    }

    #[test]
    fn fit_defaults_to_cover_and_contain_is_explicit() {
        assert_eq!(object_fit(None), "cover");
        assert_eq!(object_fit(Some(ImageFit::Cover)), "cover");
        assert_eq!(object_fit(Some(ImageFit::Contain)), "contain");
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type_of("a.png"), "image/png");
        assert_eq!(content_type_of("a.JPEG"), "image/jpeg");
        assert_eq!(content_type_of("a.svg"), "image/svg+xml");
        assert_eq!(content_type_of("a.bin"), "application/octet-stream");
    }

    #[test]
    fn a_data_url_is_kept_and_a_remote_url_is_refused_by_name() {
        let ok = src_of_value("data:image/png;base64,AAAA").expect("data URL");
        assert!(ok.starts_with("data:image/"), "{ok}");
        assert_eq!(
            src_of_value("https://evil.example/x.png"),
            Err("image_remote_refused")
        );
        assert_eq!(
            src_of_value("//evil.example/x.png"),
            Err("image_remote_refused")
        );
        assert_eq!(src_of_value("assets/logo.png"), Err("image_invalid_source"));
    }

    #[test]
    fn a_module_file_resolves_like_html_and_refuses_a_remote_url() {
        assert_eq!(
            image_resolve("m", "1.0.0", "assets/logo.png").as_deref(),
            Ok("/modules/m/1.0.0/assets/logo.png")
        );
        assert!(image_resolve("m", "1.0.0", "../../secret.png").is_err());
        assert!(image_resolve("m", "1.0.0", "https://evil.example/x.png").is_err());
    }
}
