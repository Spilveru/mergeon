# gpui-pre 0.3.7 con cuatro cambios

> **Licencia (Apache-2.0):** Este directorio es una copia modificada de `gpui-pre 0.3.7`
> (Zed Industries, Inc., Apache License 2.0, ver `LICENSE-APACHE`). Las cuatro modificaciones
> introducidas por los colaboradores de Mergeon también se publican bajo **Apache-2.0** para
> mantener compatibilidad total con upstream Zed.

Copia de `src/` de gpui-pre 0.3.7 (crates.io) sin ejemplos ni tests de
integración (se quitaron sus tablas del `Cargo.toml`), enlazada desde
`[patch.crates-io]` en `native/Cargo.toml`.

Primer cambio: `LineWrapper::is_word_char` trata `/` como parte de la palabra
(`src/text_system/line_wrapper.rs`). GPUI partía antes de cada `/`, así que
«Merge feature/search into main» quedaba «Merge feature» / «/search into
main»; Chromium no corta ahí. Al subir de versión de gpui-pre hay que volver a
aplicarlo o quitar el parche si llega a upstream.

Segundo cambio: una palabra que no cabe en una línea entera (una ruta como
`apps/api/src/common/multer-error.interceptor.ts`) se corta después de `/`,
`\`, `-` o `_` en vez de en una letra cualquiera (`…multer-error.inte` /
`rceptor.ts`). Es un corte de emergencia: solo se usa si la línea no tiene un
espacio donde cortar. Está en `LineWrapper::breaks_after` y en las dos copias
del algoritmo de corte: `LineLayout::compute_wrap_boundaries`
(`src/text_system/line_layout.rs`, la que se usa al pintar) y
`LineWrapper::wrap_line` / `truncate_wrapped_line`. De paso `wrap_line` ya no
deja `prev_c` desfasado tras devolver un corte.

Tercer cambio: pulsar cualquier botón del ratón cancela también un tooltip que
aún espera su retardo (`ActiveTooltip::WaitingForShow` en
`clear_active_tooltip_if_not_hoverable`, `src/elements/div.rs`). Antes solo se
cerraba el que ya estaba visible: un clic derecho sobre una pestaña justo al
llegar a ella abría el menú contextual y, al cumplirse el retardo, el tooltip
con la ruta aparecía encima de su primer ítem.

Cuarto cambio: la consulta de zonas de control de ventana
(`on_hit_test_window_control` en `src/window.rs`, la que responde a
WM_NCHITTEST en Windows) solo mira los hitboxes que reciben el ratón
(`ids[..hover_hitbox_count]`), igual que el resto de eventos. Antes miraba
todos los que hay bajo el puntero: una pestaña con `block_mouse_except_scroll`
encima de la zona de arrastre de la barra de título devolvía `Drag`, Windows
contestaba HTCAPTION y la pestaña no recibía ni el clic ni el de su ×. Para
probarlo se añadió `TestAppContext::simulate_hit_test_window_control` (y
`TestWindow::simulate_hit_test_window_control`).
