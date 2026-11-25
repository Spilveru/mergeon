//! Avatares de GitHub.
//!
//! En la app original son `<img>` remotos con un ícono local de respaldo
//! cuando la imagen falla (`failedAvatars`). Aquí cada URL se descarga una
//! sola vez en un hilo de fondo y se guarda decodificada; mientras llega, o si
//! falla, quien pinta usa su ícono de respaldo.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Global, Image, ImageFormat, IntoElement,
    ObjectFit, Pixels, Rems, Styled as _, StyledImage as _, img,
};
use mergeon_github::{HttpMethod, HttpRequest, HttpTransport as _, UreqTransport};
use mergeon_ui::ActivePaletteExt as _;

/// Descarga el contenido de una URL de avatar. Se llama desde un hilo de
/// fondo. La cáscara no necesita instalar ninguno: por defecto se usa HTTP.
pub type AvatarFetcher = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

#[derive(Clone)]
enum AvatarState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

/// Tope de avatares guardados (decodificados en memoria). Con el selector
/// de repositorios de GitHub cada dueño distinto deja el suyo; al pasarse se
/// olvidan los que hace más que no se pintan.
const MAX_AVATARS: usize = 200;

struct AvatarEntry {
    state: AvatarState,
    /// Cuándo se pidió por última vez (reloj propio de la caché).
    used_at: u64,
}

/// Avatares ya pedidos, por URL.
pub struct AvatarCache {
    entries: HashMap<String, AvatarEntry>,
    clock: u64,
    fetcher: AvatarFetcher,
}

/// Deja como mucho `max` entradas quitando las usadas hace más tiempo. Las
/// que aún se descargan se quedan: su descarga volvería a guardarlas.
fn evict_least_recently_used(entries: &mut HashMap<String, AvatarEntry>, max: usize) {
    if entries.len() <= max {
        return;
    }
    let mut candidates: Vec<(u64, String)> = entries
        .iter()
        .filter(|(_, entry)| !matches!(entry.state, AvatarState::Loading))
        .map(|(url, entry)| (entry.used_at, url.clone()))
        .collect();
    candidates.sort_unstable();
    let excess = entries.len() - max;
    for (_, url) in candidates.into_iter().take(excess) {
        entries.remove(&url);
    }
}

struct GlobalAvatarCache(Entity<AvatarCache>);

impl Global for GlobalAvatarCache {}

fn http_fetcher() -> AvatarFetcher {
    let transport = UreqTransport::new();
    Arc::new(move |url| {
        let response = transport
            .send(&HttpRequest {
                method: HttpMethod::Get,
                url: url.to_string(),
                headers: Vec::new(),
                body: None,
                timeout: Duration::from_secs(15),
            })
            .map_err(|error| error.to_string())?;
        if response.is_ok() {
            Ok(response.body)
        } else {
            Err(format!("HTTP {}", response.status))
        }
    })
}

/// Formato de la imagen por sus primeros bytes; `None` si no se reconoce.
fn sniff_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
        let head = head.trim_start();
        (head.starts_with("<svg") || head.starts_with("<?xml")).then_some(ImageFormat::Svg)
    }
}

impl AvatarCache {
    /// La caché de la app; se crea la primera vez que se pide.
    pub fn global(cx: &mut App) -> Entity<AvatarCache> {
        if let Some(global) = cx.try_global::<GlobalAvatarCache>() {
            return global.0.clone();
        }
        let cache = cx.new(|_| AvatarCache {
            entries: HashMap::new(),
            clock: 0,
            fetcher: http_fetcher(),
        });
        cx.set_global(GlobalAvatarCache(cache.clone()));
        cache
    }

    /// La imagen de `url` si ya llegó. La primera vez que se pide una URL
    /// empieza su descarga; quien observe la caché se entera cuando termina.
    fn image(&mut self, url: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(url) {
            entry.used_at = self.clock;
            return match &entry.state {
                AvatarState::Ready(image) => Some(image.clone()),
                AvatarState::Loading | AvatarState::Failed => None,
            };
        }

        self.entries.insert(
            url.to_string(),
            AvatarEntry {
                state: AvatarState::Loading,
                used_at: self.clock,
            },
        );
        evict_least_recently_used(&mut self.entries, MAX_AVATARS);
        let fetcher = self.fetcher.clone();
        let url = url.to_string();
        let download = cx.background_spawn({
            let url = url.clone();
            async move { fetcher(&url) }
        });
        cx.spawn(async move |this, cx| {
            let state = match download.await {
                Ok(bytes) => match sniff_format(&bytes) {
                    Some(format) => AvatarState::Ready(Arc::new(Image::from_bytes(format, bytes))),
                    None => AvatarState::Failed,
                },
                Err(error) => {
                    log::debug!("avatar {url}: {error}");
                    AvatarState::Failed
                }
            };
            this.update(cx, |this, cx| {
                // Se pudo olvidar mientras se descargaba (o vaciar la caché).
                if let Some(entry) = this.entries.get_mut(&url) {
                    entry.state = state;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
    }

    /// `failedAvatars.has(url)`.
    pub fn has_failed(&self, url: &str) -> bool {
        self.entries
            .get(url)
            .is_some_and(|entry| matches!(entry.state, AvatarState::Failed))
    }
}

/// Sustituye la descarga de avatares (tests y galería) y olvida lo ya pedido.
pub fn set_avatar_fetcher(fetcher: AvatarFetcher, cx: &mut App) {
    AvatarCache::global(cx).update(cx, |cache, cx| {
        cache.fetcher = fetcher;
        cache.entries.clear();
        cx.notify();
    });
}

/// El avatar de `url` como `<img class="object-cover">` cuadrado de `size`
/// con borde, o `None` si no hay URL, aún no llegó o falló: quien llama pinta
/// entonces su ícono de respaldo.
pub(crate) fn avatar(
    url: Option<&str>,
    size: Rems,
    radius: Pixels,
    cx: &mut App,
) -> Option<AnyElement> {
    let url = url.filter(|url| !url.is_empty())?;
    let image = AvatarCache::global(cx).update(cx, |cache, cx| cache.image(url, cx))?;
    Some(
        img(image)
            .size(size)
            .flex_shrink_0()
            .rounded(radius)
            .border_1()
            .border_color(cx.palette().border)
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(state: AvatarState, used_at: u64) -> AvatarEntry {
        AvatarEntry { state, used_at }
    }

    #[test]
    fn forgets_the_least_recently_used_avatars_past_the_limit() {
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, vec![0x89, b'P']));
        let mut entries = HashMap::from([
            ("a".to_string(), entry(AvatarState::Ready(image.clone()), 1)),
            ("b".to_string(), entry(AvatarState::Loading, 2)),
            ("c".to_string(), entry(AvatarState::Failed, 3)),
            ("d".to_string(), entry(AvatarState::Ready(image), 4)),
        ]);

        evict_least_recently_used(&mut entries, 4);
        assert_eq!(entries.len(), 4);

        evict_least_recently_used(&mut entries, 2);
        // "a" y "c" eran los más viejos que no se están descargando.
        let mut kept: Vec<_> = entries.keys().cloned().collect();
        kept.sort();
        assert_eq!(kept, ["b", "d"]);

        // Lo que se descarga no se quita aunque sobre.
        evict_least_recently_used(&mut entries, 0);
        assert_eq!(entries.keys().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn recognises_the_formats_github_serves() {
        assert_eq!(
            sniff_format(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a]),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            sniff_format(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(sniff_format(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(
            sniff_format(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            sniff_format(b"  <svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(sniff_format(b"<!doctype html><html>"), None);
        assert_eq!(sniff_format(b""), None);
    }
}
