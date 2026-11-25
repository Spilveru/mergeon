//! Leer las dos versiones de una imagen cambiada para el visor de imágenes
//! (`mergeon_view_diff::ImageDiffViewer`): git y disco por el modelo, en
//! segundo plano, y medir y preparar los bytes también fuera del hilo de la
//! interfaz.

use gpui_kit::{App, AppContext as _, Entity, Task};
use mergeon_app::AppModel;
use mergeon_backend::{ImageBlob, ImagePairRequest};
use mergeon_view_diff::{ImageDiff, ImageDiffViewer, ImageVersion};

/// Pone el visor en "Loading image…" y lee las versiones que pide `request`.
/// El resultado se enseña con [`show`] (quien lo pide comprueba antes que
/// sigue siendo lo que quiere ver).
pub(crate) fn load_image_pair(
    model: &Entity<AppModel>,
    viewer: &Entity<ImageDiffViewer>,
    repo_path: &str,
    path: &str,
    request: ImagePairRequest,
    cx: &mut App,
) -> Task<Result<ImageDiff, String>> {
    viewer.update(cx, |viewer, cx| viewer.set_loading(cx));
    let read = model.update(cx, |model, cx| {
        model.read_image_pair(repo_path, request, cx)
    });
    let path = path.to_string();
    cx.spawn(async move |cx| {
        let pair = read.await.map_err(|error| error.message().to_string())?;
        Ok(cx
            .background_spawn(async move {
                let version = |blob: ImageBlob| {
                    ImageVersion::new(&path, blob.bytes, blob.size, blob.too_large)
                };
                ImageDiff {
                    before: pair.before.map(version),
                    after: pair.after.map(version),
                }
            })
            .await)
    })
}

/// Lo leído, al visor.
pub(crate) fn show(
    viewer: &Entity<ImageDiffViewer>,
    images: Result<ImageDiff, String>,
    cx: &mut App,
) {
    viewer.update(cx, |viewer, cx| match images {
        Ok(images) => viewer.set_images(images, cx),
        Err(error) => viewer.set_error(error, cx),
    });
}
