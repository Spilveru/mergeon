//! Imágenes antes y después: en lugar de "Binary file changed", las dos
//! versiones de una imagen lado a lado con su tamaño en píxeles y en bytes.
//! Añadida: solo "After"; borrada: solo "Before".
//!
//! Leer los bytes es cosa de quien aloja el visor (de git o del disco, fuera
//! del hilo de la interfaz). Aquí se preparan ([`ImageVersion::new`], también
//! fuera del hilo de la interfaz: mide la cabecera y calcula la huella de los
//! bytes) y se pintan con [`ImageDiffViewer`]. GPUI decodifica en segundo
//! plano; el visor suelta de su caché las imágenes que deja de mostrar.
//!
//! Un SVG es imagen y también texto: el conmutador "Image | Text"
//! ([`image_text_toggle`]) deja leer el diff de su XML en lugar de las dos
//! versiones pintadas. Quien aloja el visor pregunta con
//! [`ImageDiffViewer::shows_image`] qué pintar.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, FontWeight, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, TestSupportExt as _, Window,
    div, img, px, rems,
};
use mergeon_model::format::format_size;
use mergeon_ui::{ActivePaletteExt as _, Radius, SpinnerColor, SpinnerSize, TabItem, Tabs};

/// Extensiones que se enseñan como imagen.
pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"];

/// El formato de imagen de una ruta, por su extensión.
pub fn image_format_for_path(path: &str) -> Option<ImageFormat> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        "ico" => ImageFormat::Ico,
        "svg" => ImageFormat::Svg,
        _ => return None,
    })
}

/// Si el archivo se enseña como imagen (antes/después) y no como texto.
pub fn is_image_path(path: &str) -> bool {
    image_format_for_path(path).is_some()
}

/// Si el archivo es un SVG: se enseña como imagen, pero también tiene un diff
/// de texto que leer ("Image | Text").
pub fn is_svg_path(path: &str) -> bool {
    image_format_for_path(path) == Some(ImageFormat::Svg)
}

fn u16_be(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn u16_le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn u24_le(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}

fn u32_be(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Ancho y alto en píxeles leyendo solo la cabecera (PNG, JPEG, GIF, WebP,
/// BMP, ICO) o los atributos de la raíz (SVG). `None` si no se reconoce.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((u32_be(bytes, 16)?, u32_be(bytes, 20)?));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((u16_le(bytes, 6)?, u16_le(bytes, 8)?));
    }
    if bytes.starts_with(b"BM") {
        let width = u32_le(bytes, 18)? as i32;
        let height = u32_le(bytes, 22)? as i32;
        return Some((width.unsigned_abs(), height.unsigned_abs()));
    }
    if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() >= 8 {
        // ICO: la primera imagen; 0 quiere decir 256.
        let side = |byte: u8| if byte == 0 { 256 } else { u32::from(byte) };
        return Some((side(bytes[6]), side(bytes[7])));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return match bytes.get(12..16)? {
            b"VP8 " => Some((u16_le(bytes, 26)? & 0x3fff, u16_le(bytes, 28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32_le(bytes, 21)?;
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((u24_le(bytes, 24)? + 1, u24_le(bytes, 27)? + 1)),
            _ => None,
        };
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return jpeg_dimensions(bytes);
    }
    svg_dimensions(bytes)
}

/// Recorre los segmentos del JPEG hasta el `SOFn`, que lleva las medidas.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xff {
            at += 1;
            continue;
        }
        let marker = bytes[at + 1];
        match marker {
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Some((u16_be(bytes, at + 7)?, u16_be(bytes, at + 5)?));
            }
            0xd0..=0xd9 | 0x01 | 0xff => at += if marker == 0xff { 1 } else { 2 },
            _ => at += 2 + u16_be(bytes, at + 2)? as usize,
        }
    }
    None
}

/// `width` y `height` de la raíz `<svg>` (en px o sin unidad) o, si no
/// están, el `viewBox`.
fn svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let start = head.find("<svg")?;
    let tag = &head[start..start + head[start..].find('>')?];
    let attribute = |name: &str| -> Option<&str> {
        let mut rest = tag;
        loop {
            let at = rest.find(name)?;
            let before = rest[..at].chars().last();
            rest = &rest[at + name.len()..];
            if !before.is_some_and(char::is_whitespace) {
                continue;
            }
            let value = rest.trim_start().strip_prefix('=')?.trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &value[1..];
            return Some(&value[..value.find(quote)?]);
        }
    };
    let number = |value: &str| -> Option<f32> {
        let value = value.trim().trim_end_matches("px");
        value.parse::<f32>().ok().filter(|number| *number > 0.)
    };
    let size = |width: f32, height: f32| Some((width.round() as u32, height.round() as u32));
    if let (Some(width), Some(height)) = (
        attribute("width").and_then(number),
        attribute("height").and_then(number),
    ) {
        return size(width, height);
    }
    let view_box: Vec<f32> = attribute("viewBox")?
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match view_box.as_slice() {
        [_, _, width, height] if *width > 0. && *height > 0. => size(*width, *height),
        _ => None,
    }
}

/// Una versión de la imagen, lista para pintar.
#[derive(Debug, Clone)]
pub struct ImageVersion {
    /// `None` si no se cargó (demasiado grande).
    pub image: Option<Arc<Image>>,
    /// Bytes del archivo.
    pub size: u64,
    /// Ancho y alto en píxeles, si la cabecera se entiende.
    pub dimensions: Option<(u32, u32)>,
    pub too_large: bool,
}

impl ImageVersion {
    /// Prepara los bytes de `path`. Recorre los bytes (la huella con la que
    /// GPUI la guarda): llamarlo fuera del hilo de la interfaz. Con
    /// `too_large` no se carga: `bytes` puede traer solo el principio del
    /// archivo, y de él salen las medidas.
    pub fn new(path: &str, bytes: Vec<u8>, size: u64, too_large: bool) -> Self {
        if too_large {
            return Self {
                image: None,
                size,
                dimensions: image_dimensions(&bytes),
                too_large: true,
            };
        }
        let dimensions = image_dimensions(&bytes);
        let format = image_format_for_path(path).unwrap_or(ImageFormat::Png);
        Self {
            image: Some(Arc::new(Image::from_bytes(format, bytes))),
            size,
            dimensions,
            too_large: false,
        }
    }

    /// "640 × 480 px · 12.4 KB" (sin medidas si la cabecera no se entiende).
    pub fn caption(&self) -> String {
        let bytes = format_size(self.size as f64);
        match self.dimensions {
            Some((width, height)) => format!("{width} \u{d7} {height} px \u{b7} {bytes}"),
            None => bytes,
        }
    }
}

/// Las dos versiones. `None` en un lado: no existía (añadida) o ya no existe
/// (borrada).
#[derive(Debug, Clone, Default)]
pub struct ImageDiff {
    pub before: Option<ImageVersion>,
    pub after: Option<ImageVersion>,
}

impl ImageDiff {
    fn images(&self) -> impl Iterator<Item = &Arc<Image>> {
        [&self.before, &self.after]
            .into_iter()
            .flatten()
            .filter_map(|version| version.image.as_ref())
    }
}

/// Lo que enseña el visor.
#[derive(Debug, Clone, Default)]
enum ImageState {
    #[default]
    Empty,
    Loading,
    Ready(ImageDiff),
    Failed(SharedString),
}

/// Visor de una imagen cambiada: las dos versiones lado a lado. Quien lo
/// aloja lo crea una vez, le pasa lo que lee con los `set_*` y lo pinta como
/// hijo; ocupa todo su contenedor.
pub struct ImageDiffViewer {
    state: ImageState,
    /// "Text" elegido en el conmutador de un SVG: quien aloja el visor pinta
    /// el diff del XML en su lugar. Sobrevive a los cambios de archivo, como
    /// el resto de interruptores del diff.
    svg_as_text: bool,
}

impl ImageDiffViewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Al cerrarse, las imágenes que pintaba salen de la caché de GPUI.
        cx.on_release(|this, cx| this.release_images(cx)).detach();
        Self {
            state: ImageState::Empty,
            svg_as_text: false,
        }
    }

    /// Un SVG se ve como su diff de texto ("Text") y no pintado ("Image", lo
    /// predeterminado).
    pub fn svg_as_text(&self) -> bool {
        self.svg_as_text
    }

    pub fn set_svg_as_text(&mut self, as_text: bool, cx: &mut Context<Self>) {
        if self.svg_as_text != as_text {
            self.svg_as_text = as_text;
            cx.notify();
        }
    }

    /// Si `path` se pinta con este visor (antes y después) y no con el diff
    /// de texto: una imagen, salvo un SVG con "Text" elegido.
    pub fn shows_image(&self, path: &str) -> bool {
        is_image_path(path) && !(self.svg_as_text && is_svg_path(path))
    }

    /// Mientras se leen las versiones.
    pub fn set_loading(&mut self, cx: &mut Context<Self>) {
        self.replace(ImageState::Loading, cx);
    }

    pub fn set_images(&mut self, diff: ImageDiff, cx: &mut Context<Self>) {
        self.replace(ImageState::Ready(diff), cx);
    }

    pub fn set_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.replace(ImageState::Failed(message.into()), cx);
    }

    /// Nada que mostrar (otro archivo).
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.replace(ImageState::Empty, cx);
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, ImageState::Loading)
    }

    /// Las versiones a la vista, si ya están.
    pub fn images(&self) -> Option<&ImageDiff> {
        match &self.state {
            ImageState::Ready(diff) => Some(diff),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&SharedString> {
        match &self.state {
            ImageState::Failed(message) => Some(message),
            _ => None,
        }
    }

    fn replace(&mut self, state: ImageState, cx: &mut Context<Self>) {
        self.release_images(cx);
        self.state = state;
        cx.notify();
    }

    fn release_images(&mut self, cx: &mut App) {
        if let ImageState::Ready(diff) = &self.state {
            for image in diff.images() {
                image.clone().remove_asset(cx);
            }
        }
    }
}

/// El conmutador "Image | Text" de un SVG (el segmentado de Mergeon), para
/// la cabecera del diff. Cambia [`ImageDiffViewer::svg_as_text`]; quien aloja
/// el visor lo lee al pintar y se vuelve a pintar con el aviso del visor.
/// Solo tiene sentido si la ruta es de un SVG ([`is_svg_path`]).
pub fn image_text_toggle(viewer: &Entity<ImageDiffViewer>, cx: &App) -> Tabs {
    let as_text = viewer.read(cx).svg_as_text();
    let viewer = viewer.downgrade();
    mergeon_ui::tabs("svg-view")
        .item(TabItem::new(SVG_AS_IMAGE, "Image"))
        .item(TabItem::new(SVG_AS_TEXT, "Text"))
        .selected(if as_text { SVG_AS_TEXT } else { SVG_AS_IMAGE })
        .on_change(move |key, _, cx| {
            let as_text = key.as_ref() == SVG_AS_TEXT;
            viewer
                .update(cx, |viewer, cx| viewer.set_svg_as_text(as_text, cx))
                .ok();
        })
}

/// Claves del conmutador; sus botones se llaman `tab-svg-image` y
/// `tab-svg-text`.
const SVG_AS_IMAGE: &str = "svg-image";
const SVG_AS_TEXT: &str = "svg-text";

/// Una mitad: título, medidas y la imagen centrada (nunca más grande que su
/// tamaño real).
fn pane(id: &'static str, title: &'static str, version: &ImageVersion, cx: &App) -> AnyElement {
    let palette = *cx.palette();
    let caption = SharedString::from(version.caption());
    let body: AnyElement = match &version.image {
        Some(image) => img(image.clone())
            // Con id, `img` guarda estado entre frames: sin él GPUI no
            // apunta cuándo empezó a decodificar y nunca pinta `with_loading`
            // (una imagen de varios MB dejaba el recuadro vacío casi un
            // segundo).
            .id("picture")
            .size_full()
            // GPUI rasteriza los SVG a 2× (`SMOOTH_SVG_SCALE_FACTOR`) y sin
            // este techo `ObjectFit::ScaleDown` los pintaba al doble de su
            // tamaño declarado; acotarlos a sus medidas naturales conserva la
            // nitidez al escalar y respeta el 1:1 en cualquier formato.
            .when_some(version.dimensions, |el, (width, height)| {
                el.max_w(px(width as f32)).max_h(px(height as f32))
            })
            .object_fit(ObjectFit::ScaleDown)
            .with_fallback({
                let color = palette.muted_foreground;
                move || {
                    div()
                        .text_color(color)
                        .child("Can't display this image")
                        .into_any_element()
                }
            })
            .with_loading({
                // GPUI lo pinta si la decodificación pasa de `LOADING_DELAY`
                // (200 ms): las imágenes pequeñas no parpadean.
                let color = palette.muted_foreground;
                let decoding_id = ElementId::Name(format!("{id}-decoding").into());
                move || {
                    div()
                        .id(decoding_id.clone())
                        .test_support()
                        .aria_label("Decoding image…")
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .text_color(color)
                        .child(
                            mergeon_ui::spinner()
                                .size(SpinnerSize::Sm)
                                .color(SpinnerColor::Primary),
                        )
                        .child("Decoding image…")
                        .into_any_element()
                }
            })
            .into_any_element(),
        None => div()
            .text_color(palette.muted_foreground)
            .child(format!(
                "Too large to preview ({})",
                format_size(version.size as f64)
            ))
            .into_any_element(),
    };
    div()
        .id(id)
        .test_support()
        .aria_label(caption.clone())
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .font_family(crate::code_font_family(cx))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.text_3)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(rems(0.75))
                        .text_color(palette.text_2)
                        .child(caption),
                ),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .p(px(12.))
                .bg(palette.bg_inset)
                .border_1()
                .border_color(palette.border_hair)
                .rounded(Radius::Lg.px(cx))
                .overflow_hidden()
                .child(body),
        )
        .into_any_element()
}

impl Render for ImageDiffViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .id("image-diff")
            .test_support()
            .flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .gap(px(12.))
            .p(px(12.))
            .bg(palette.card);
        let note = |id: &'static str, text: SharedString, spinner: bool| {
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_color(palette.muted_foreground)
                .when(spinner, |this| {
                    this.child(
                        mergeon_ui::spinner()
                            .size(SpinnerSize::Sm)
                            .color(SpinnerColor::Primary),
                    )
                })
                .child(text)
        };
        match &self.state {
            ImageState::Empty => root,
            ImageState::Loading => root.child(note("image-loading", "Loading image…".into(), true)),
            ImageState::Failed(message) => root.child(note("image-error", message.clone(), false)),
            ImageState::Ready(diff) => match (&diff.before, &diff.after) {
                (None, None) => root.child(note("image-missing", "No image to show".into(), false)),
                (before, after) => root
                    .when_some(before.as_ref(), |root, before| {
                        root.child(pane(
                            "image-before",
                            if after.is_some() { "BEFORE" } else { "DELETED" },
                            before,
                            cx,
                        ))
                    })
                    .when_some(after.as_ref(), |root, after| {
                        root.child(pane(
                            "image-after",
                            if before.is_some() { "AFTER" } else { "ADDED" },
                            after,
                            cx,
                        ))
                    }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_which_paths_are_images() {
        for path in [
            "a.png",
            "dir/B.JPG",
            "c.jpeg",
            "d.gif",
            "e.webp",
            "f.bmp",
            "g.ico",
            "h.svg",
        ] {
            assert!(is_image_path(path), "{path}");
        }
        for path in ["a.txt", "png", "a.png.orig", "a.tiff", "Makefile"] {
            assert!(!is_image_path(path), "{path}");
        }
        assert_eq!(image_format_for_path("x.JPEG"), Some(ImageFormat::Jpeg));
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn reads_the_size_from_the_header() {
        assert_eq!(image_dimensions(&png(640, 480)), Some((640, 480)));

        let mut gif = b"GIF89a".to_vec();
        gif.extend(320u16.to_le_bytes());
        gif.extend(200u16.to_le_bytes());
        assert_eq!(image_dimensions(&gif), Some((320, 200)));

        let mut bmp = vec![b'B', b'M'];
        bmp.resize(18, 0);
        bmp.extend(16i32.to_le_bytes());
        bmp.extend((-9i32).to_le_bytes());
        assert_eq!(image_dimensions(&bmp), Some((16, 9)));

        assert_eq!(
            image_dimensions(&[0, 0, 1, 0, 1, 0, 0, 48]),
            Some((256, 48))
        );

        // JPEG: un APP0 y luego el SOF0 con alto 100 y ancho 150.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend([0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x64, 0x00, 0x96, 0x03]);
        assert_eq!(image_dimensions(&jpeg), Some((150, 100)));

        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.resize(24, 0);
        webp.extend([0x3f, 0x01, 0x00, 0xc7, 0x00, 0x00]);
        assert_eq!(image_dimensions(&webp), Some((320, 200)));

        assert_eq!(image_dimensions(b"not an image"), None);
        assert_eq!(image_dimensions(&[]), None);
    }

    #[test]
    fn reads_the_size_of_an_svg() {
        let sized = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="24px" height="16"><path/></svg>"#;
        assert_eq!(image_dimensions(sized), Some((24, 16)));
        let boxed = br#"<svg viewBox="0 0 100 50" stroke-width="2"></svg>"#;
        assert_eq!(image_dimensions(boxed), Some((100, 50)));
        // `stroke-width` no es `width`.
        let neither = br#"<svg stroke-width="2"></svg>"#;
        assert_eq!(image_dimensions(neither), None);
    }

    #[test]
    fn the_caption_says_pixels_and_bytes() {
        let version = ImageVersion::new("a.png", png(640, 480), 2048, false);
        assert_eq!(version.dimensions, Some((640, 480)));
        assert!(version.image.is_some());
        assert_eq!(
            version.caption(),
            format!("640 \u{d7} 480 px \u{b7} {}", format_size(2048.))
        );

        let big = ImageVersion::new("a.png", Vec::new(), 30 * 1024 * 1024, true);
        assert!(big.image.is_none());
        assert_eq!(big.caption(), format_size(30. * 1024. * 1024.));

        // Demasiado grande para cargarla, pero con su cabecera: se mide igual.
        let measured = ImageVersion::new("a.png", png(8000, 6000), 120 * 1024 * 1024, true);
        assert!(measured.image.is_none());
        assert_eq!(measured.dimensions, Some((8000, 6000)));
        assert_eq!(
            measured.caption(),
            format!(
                "8000 \u{d7} 6000 px \u{b7} {}",
                format_size(120. * 1024. * 1024.)
            )
        );
    }

    #[test]
    fn svg_is_the_only_image_with_a_text_view() {
        assert!(is_svg_path("img/logo.SVG"));
        assert!(!is_svg_path("img/logo.png"));
        assert!(!is_svg_path("logo.svg.txt"));
    }
}
