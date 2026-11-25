//! Recursos embebidos: íconos Lucide (catálogo de gpui-kit) y fuentes propias.

use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::{App, AssetSource, Result, SharedString};

/// Fuente de recursos de la app: el catálogo de íconos Lucide de gpui-kit y su
/// versión de trazo fino para [`crate::icon`].
pub struct Assets;

/// Prefijo de los íconos de trazo fino. Señal: trazo de 1,5px con terminal
/// recto (`stroke-linecap: square`, `stroke-linejoin: miter`). Los archivos son
/// los mismos de `icons/`, con el trazo reescrito al cargarlos.
pub(crate) const THIN_ICONS: &str = "icons@thin/";
const ICONS: &str = "icons/";

/// Ruta de la versión de trazo fino de un ícono del catálogo.
pub(crate) fn thin_icon_path(path: &str) -> Option<SharedString> {
    path.strip_prefix(ICONS)
        .map(|name| SharedString::from(format!("{THIN_ICONS}{name}")))
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path.strip_prefix(THIN_ICONS) else {
            return AllAssets.load(path);
        };
        // GPUI guarda el resultado rasterizado por ruta y tamaño, así que esto
        // corre una vez por ícono y tamaño.
        let Some(svg) = AllAssets.load(&format!("{ICONS}{name}"))? else {
            return Ok(None);
        };
        let svg = String::from_utf8_lossy(&svg)
            .replacen("stroke-width=\"2\"", "stroke-width=\"1.5\"", 1)
            .replacen("stroke-linecap=\"round\"", "stroke-linecap=\"square\"", 1)
            .replacen("stroke-linejoin=\"round\"", "stroke-linejoin=\"miter\"", 1);
        Ok(Some(Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        AllAssets.list(path)
    }
}

/// Geist y Geist Mono (Vercel, SIL OFL 1.1, ver `assets/fonts/OFL.txt`):
/// instancias estáticas de los pesos que usa la UI (tema «Señal»). Geist para la
/// interfaz; Geist Mono para hashes, ramas, cifras y código (con su cursiva para
/// los comentarios del diff).
const FONTS: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/GeistMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Italic.ttf"),
];

pub(crate) fn register_fonts(cx: &mut App) {
    let fonts = FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        log::error!("no se pudieron registrar las fuentes embebidas: {error}");
    }
}
