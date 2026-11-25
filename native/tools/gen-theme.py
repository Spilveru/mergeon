#!/usr/bin/env python3
"""Genera el tema «Señal» de Mergeon para gpui-kit.

La fuente de verdad son los tokens de Señal (claro y oscuro), copiados en hex
final (ya recortados a la gama sRGB) de `tokens.js` del diseño
(`mergeon-design/senal/`, tabla 3.4 de su DESIGN.md). Para cambiar un color,
cambiarlo allí, copiar aquí el hex nuevo y volver a correr este script.

Salidas:
  crates/mergeon-ui/assets/themes/mergeon.json   tema claro y oscuro para gpui-kit
  crates/mergeon-ui/src/theme/palette.rs         tokens propios (Señal + nombres heredados)

Los componentes y vistas se escribieron con los nombres de shadcn (`primary`,
`card`, `muted_foreground`...). Esos nombres se conservan como alias de los
tokens de Señal (ver `legacy`), así que el tema cambia sin tocar cada vista; lo
nuevo (selección en tinta, carriles, diff...) usa los nombres de Señal.

Uso: python3 native/tools/gen-theme.py
"""
import json, os

SENAL = {
    "light": {
        "bg-sunken": "#f7f7f7", "bg-base": "#ffffff", "bg-raised": "#ffffff", "bg-inset": "#f9f9f9",
        "bg-hover": "#f1f1f1", "bg-pressed": "#e8e8e8", "bg-scrim": "#131313",
        "sel-bg": "#404040", "sel-fg": "#fafafa", "sel-fg-2": "#bebebe", "sel-inactive": "#e6e6e6",
        "sel-add": "#6ed889", "sel-del": "#ff9592",
        "text-1": "#131313", "text-2": "#4d4d4d", "text-3": "#6d6d6d", "text-disabled": "#a4a4a4",
        "border-hair": "#e4e4e4", "border-base": "#d2d2d2", "border-control": "#808080", "border-ink": "#131313",
        "accent": "#f55900", "accent-hover": "#fa7520", "accent-active": "#e74e05", "accent-fg": "#131313",
        "accent-text": "#b43d00", "accent-soft": "#ffede1", "accent-soft-border": "#fdc7a7", "focus": "#de4a00",
        "success": "#218a45", "success-text": "#197037", "success-soft": "#e3fae6",
        "warning": "#f4b93c", "warning-text": "#885716", "warning-soft": "#fff4d3",
        "danger": "#cb242d", "danger-hover": "#b51e26", "danger-text": "#c22029", "danger-soft": "#ffeeed",
        "info": "#166eb9", "info-text": "#0f66ac", "info-soft": "#e9f4ff",
        "diff-add-bg": "#e5fbe9", "diff-add-gutter": "#d3f4d8", "diff-add-word": "#afeaba", "diff-add-sign": "#197037",
        "diff-del-bg": "#fff0ef", "diff-del-gutter": "#ffe1e1", "diff-del-word": "#ffcfcf", "diff-del-sign": "#c22029",
        "diff-hunk-bg": "#f2f2f2", "diff-hunk-text": "#636363",
        "lane-0": "#cf652f", "lane-1": "#a68300", "lane-2": "#5d9b34", "lane-3": "#009d8a",
        "lane-4": "#0096bd", "lane-5": "#6580e1", "lane-6": "#a968c5", "lane-7": "#cd5b84",
    },
    "dark": {
        "bg-sunken": "#101010", "bg-base": "#171717", "bg-raised": "#202020", "bg-inset": "#1c1c1c",
        "bg-hover": "#232323", "bg-pressed": "#2b2b2b", "bg-scrim": "#000000",
        "sel-bg": "#3a3a3a", "sel-fg": "#fafafa", "sel-fg-2": "#bebebe", "sel-inactive": "#2a2a2a",
        "sel-add": "#7cdd93", "sel-del": "#ffa09c",
        "text-1": "#eeeeee", "text-2": "#b2b2b2", "text-3": "#8d8d8d", "text-disabled": "#555555",
        "border-hair": "#2a2a2a", "border-base": "#383838", "border-control": "#747474", "border-ink": "#eeeeee",
        "accent": "#ff7b24", "accent-hover": "#ff9343", "accent-active": "#f36717", "accent-fg": "#0d0d0d",
        "accent-text": "#ff934a", "accent-soft": "#402012", "accent-soft-border": "#723311", "focus": "#ff8739",
        "success": "#4cb86a", "success-text": "#68ca80", "success-soft": "#162e1c",
        "warning": "#f4b93c", "warning-text": "#f1c45e", "warning-soft": "#36270e",
        "danger": "#d53136", "danger-hover": "#c52b30", "danger-text": "#fc7a73", "danger-soft": "#431d1c",
        "info": "#61a3e6", "info-text": "#79b6f4", "info-soft": "#172a3e",
        "diff-add-bg": "#112616", "diff-add-gutter": "#142e1a", "diff-add-word": "#154724", "diff-add-sign": "#68ca80",
        "diff-del-bg": "#311818", "diff-del-gutter": "#3d1d1d", "diff-del-word": "#67292b", "diff-del-sign": "#fc7a73",
        "diff-hunk-bg": "#1e1e1e", "diff-hunk-text": "#989898",
        "lane-0": "#f28f61", "lane-1": "#cda834", "lane-2": "#86bf65", "lane-3": "#02c8b0",
        "lane-4": "#22bdec", "lane-5": "#8ca7ff", "lane-6": "#cc91e7", "lane-7": "#f186a9",
    },
}

WHITE = (1.0, 1.0, 1.0, 1.0)
BLACK = (0.0, 0.0, 0.0, 1.0)
CLEAR = (0.0, 0.0, 0.0, 0.0)

def parse(hex_):
    hex_ = hex_.lstrip("#")
    return tuple(int(hex_[i:i + 2], 16) / 255 for i in (0, 2, 4)) + (1.0,)

def alpha(color, a):
    return color[:3] + (a,)

def mix(a, pa, b):
    """color-mix(in srgb, a pa%, b) con alfa premultiplicado."""
    p = pa / 100.0
    al = a[3] * p + b[3] * (1 - p)
    if al == 0:
        return CLEAR
    return tuple((a[i] * a[3] * p + b[i] * b[3] * (1 - p)) / al for i in range(3)) + (al,)

def hexa(color):
    r, g, b, a = (round(min(1, max(0, x)) * 255) for x in color)
    return f"#{r:02x}{g:02x}{b:02x}" + (f"{a:02x}" if a != 255 else "")

def senal(mode):
    """Tokens de Señal con nombres de Rust; el acento pasa a la familia `primary`."""
    s = {k: parse(v) for k, v in SENAL[mode].items()}
    rename = {
        "accent": "primary", "accent-hover": "primary-hover", "accent-active": "primary-active",
        "accent-fg": "primary-foreground", "accent-text": "primary-text",
        "accent-soft": "primary-soft", "accent-soft-border": "primary-soft-border",
        "danger": "destructive", "danger-hover": "destructive-hover",
        "danger-text": "destructive-foreground", "danger-soft": "destructive-soft",
        "success-text": "success-foreground", "warning-text": "warning-foreground",
        "info-text": "info-foreground",
    }
    return {rename.get(k, k).replace("-", "_"): v for k, v in s.items()}

def tokens(mode):
    dark = mode == "dark"
    t = senal(mode)
    # Alias heredados (nombres de shadcn con los que se escribieron las vistas).
    legacy = {
        "background": t["bg_sunken"],          # cromo de la app: barras, rail, sidebar
        "foreground": t["text_1"],
        "card": t["bg_base"],                  # listas, diff, paneles de contenido
        "card_foreground": t["text_1"],
        "popover": t["bg_raised"],
        "popover_foreground": t["text_1"],
        "secondary": t["bg_hover"],
        "secondary_foreground": t["text_1"],
        "muted": t["bg_inset"],
        "muted_foreground": t["text_3"],
        "accent": t["bg_hover"],               # hover de filas y botones fantasma
        "accent_foreground": t["text_1"],
        "border": t["border_hair"],
        "input": t["border_control"],
        "ring": t["focus"],
        "primary_soft_foreground": t["primary_text"],
        "destructive_soft_foreground": t["destructive_foreground"],
        "success_soft_foreground": t["success_foreground"],
        "warning_soft_foreground": t["warning_foreground"],
        "sidebar": t["bg_sunken"],
        "sidebar_foreground": t["text_1"],
        "sidebar_muted_foreground": t["text_3"],
        "sidebar_border": t["border_hair"],
        "sidebar_control_surface": t["bg_hover"],
        "sidebar_row_hover": t["bg_hover"],
        "sidebar_row_active": t["bg_pressed"],
        "sidebar_row_selected": t["sel_inactive"],
        "floating_border": t["border_base"],
        "dialog_glass": t["bg_raised"],
        "dialog_border": t["border_base"],
        "dialog_highlight": CLEAR,
        # Velo de diálogo: `bg-scrim` al 32 % (claro) / 60 % (oscuro).
        "dialog_backdrop": alpha(t["bg_scrim"], 0.60 if dark else 0.32),
        # Colores de las sombras de Señal (las geometrías están en style.rs).
        "shadow_raised": alpha(BLACK, 0.30 if dark else 0.04),
        "shadow_floating": alpha(BLACK, 0.60 if dark else 0.16),
        "shadow_dialog": alpha(BLACK, 0.75 if dark else 0.28),
        "emerald_400": t["success"],
        "scrollbar_thumb": mix(t["text_1"], 18, CLEAR),
        "scrollbar_thumb_hover": mix(t["text_1"], 30, CLEAR),
    }
    t.update(legacy)
    return t

def kit_theme(mode, t):
    """Tokens que leen los componentes de gpui-kit (Input, Select, Tree, menús
    de gpui-kit...) para que hablen el mismo lenguaje que los de mergeon-ui."""
    dark = mode == "dark"
    colors = {
        "background": t["bg_base"],
        "foreground": t["text_1"],
        "border": t["border_hair"],
        "input.border": t["border_control"],
        "ring": t["focus"],
        "caret": t["text_1"],
        "primary.background": t["primary"],
        "primary.foreground": t["primary_foreground"],
        "primary.hover.background": t["primary_hover"],
        "primary.active.background": t["primary_active"],
        "secondary.background": t["bg_hover"],
        "secondary.foreground": t["text_1"],
        "secondary.hover.background": t["bg_pressed"],
        "secondary.active.background": t["bg_pressed"],
        "muted.background": t["bg_inset"],
        "muted.foreground": t["text_3"],
        # gpui-kit pinta con `accent` el ítem activo de menús, paletas y listas
        # buscables: en Señal es la fila en tinta.
        "accent.background": t["sel_bg"],
        "accent.foreground": t["sel_fg"],
        "popover.background": t["bg_raised"],
        "popover.foreground": t["text_1"],
        "danger.background": t["destructive"],
        "danger.foreground": WHITE,
        "danger.hover.background": t["destructive_hover"],
        "danger.active.background": t["destructive_hover"],
        "info.background": t["info"],
        "info.foreground": WHITE,
        "success.background": t["success"],
        "success.foreground": WHITE,
        # El relleno de aviso es claro: lleva tinta encima.
        "warning.background": t["warning"],
        "warning.foreground": t["primary_foreground"],
        "button.background": t["bg_base"],
        "button.foreground": t["text_1"],
        "button.hover.background": t["bg_hover"],
        "button.active.background": t["bg_pressed"],
        "button.primary.background": t["primary"],
        "button.primary.foreground": t["primary_foreground"],
        "button.primary.hover.background": t["primary_hover"],
        "button.primary.active.background": t["primary_active"],
        "button.secondary.background": t["bg_hover"],
        "button.secondary.foreground": t["text_1"],
        "button.secondary.hover.background": t["bg_pressed"],
        "button.secondary.active.background": t["bg_pressed"],
        "button.danger.background": t["destructive"],
        "button.danger.foreground": WHITE,
        "button.danger.hover.background": t["destructive_hover"],
        "button.danger.active.background": t["destructive_hover"],
        "link": t["primary_text"],
        "link.hover": t["primary_text"],
        "link.active": t["primary_text"],
        "list.background": CLEAR,
        "list.hover.background": t["bg_hover"],
        # Fila activa: en tinta, como la selección de Señal.
        "list.active.background": t["sel_bg"],
        "list.active.border": CLEAR,
        "list.even.background": CLEAR,
        "list.head.background": t["bg_inset"],
        "table.background": CLEAR,
        "table.hover.background": t["bg_hover"],
        "table.active.background": t["sel_inactive"],
        "table.active.border": t["primary"],
        "table.even.background": CLEAR,
        "table.head.background": t["bg_inset"],
        "table.head.foreground": t["text_3"],
        "table.row.border": t["border_hair"],
        "tab.background": CLEAR,
        "tab.foreground": t["text_2"],
        "tab.active.background": CLEAR,
        "tab.active.foreground": t["text_1"],
        "tab_bar.background": t["bg_sunken"],
        "tab_bar.segmented.background": t["bg_inset"],
        "title_bar.background": t["bg_sunken"],
        "title_bar.border": t["border_hair"],
        "status_bar.background": t["bg_sunken"],
        "status_bar.border": t["border_hair"],
        "sidebar.background": t["bg_sunken"],
        "sidebar.foreground": t["text_1"],
        "sidebar.border": t["border_hair"],
        "sidebar.accent.background": t["bg_hover"],
        "sidebar.accent.foreground": t["text_1"],
        "sidebar.primary.background": t["primary"],
        "sidebar.primary.foreground": t["primary_foreground"],
        "scrollbar.background": CLEAR,
        "scrollbar.thumb.background": t["scrollbar_thumb"],
        "scrollbar.thumb.hover.background": t["scrollbar_thumb_hover"],
        "selection.background": mix(t["primary"], 30, CLEAR),
        "skeleton.background": t["bg_hover"],
        "switch.background": t["border_base"],
        "accordion.background": CLEAR,
        "group_box.background": t["bg_base"],
        "group_box.foreground": t["text_1"],
        "progress.bar.background": t["primary"],
        "slider.background": t["primary"],
        "slider.thumb.background": t["bg_base"],
        "drag.border": t["primary"],
        "drop_target.background": t["primary_soft"],
        "overlay": t["dialog_backdrop"],
        "window.border": t["border_base"],
        "base.red": t["destructive"],
        "base.red.light": t["destructive_foreground"],
        "base.green": t["success"],
        "base.green.light": t["success_foreground"],
        "base.blue": t["info"],
        "base.blue.light": t["info_foreground"],
        "base.yellow": t["warning"],
        "base.yellow.light": t["warning_foreground"],
    }
    return {
        "is_default": True,
        "name": f"Mergeon {'Dark' if dark else 'Light'}",
        "mode": mode,
        "font.family": "Geist",
        "mono_font.family": "Geist Mono",
        # El rem de la ventana es `font.size`: con 16 las utilidades de GPUI
        # (`h_8`, `gap_2`) miden lo mismo que las de Tailwind. El texto base de
        # 13px de Señal lo pone la raíz de mergeon-ui (`root.rs`).
        "font.size": 16,
        "mono_font.size": 12,
        # Señal: radios 0–6px. `--r-2` (4px) para botones e inputs; los
        # flotantes (`--r-3`, 6px) salen de `Radius::Xl` en style.rs.
        "radius": 4,
        "radius.lg": 6,
        "shadow": True,
        "colors": {key: hexa(value) for key, value in colors.items()},
        # El `Editor` de gpui-kit (resultado del resolvedor de conflictos) toma
        # fondo, medianil y línea activa de aquí; sin esto se queda con los
        # del tema claro por defecto también en oscuro.
        "highlight": {
            "editor.background": hexa(t["bg_base"]),
            "editor.foreground": hexa(t["text_1"]),
            "editor.active_line.background": hexa(t["bg_inset"]),
            "editor.line_number": hexa(t["text_3"]),
            "editor.active_line_number": hexa(t["text_1"]),
            "editor.gutter.background": hexa(t["bg_base"]),
            "syntax": {},
        },
    }

root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
palettes = {mode: tokens(mode) for mode in ("light", "dark")}

with open(os.path.join(root, "crates/mergeon-ui/assets/themes/mergeon.json"), "w") as f:
    json.dump(
        {
            "name": "Mergeon",
            "author": "Mergeon",
            "themes": [kit_theme(mode, palettes[mode]) for mode in ("light", "dark")],
        },
        f,
        indent=2,
    )
    f.write("\n")

LANES = [f"lane_{i}" for i in range(8)]
names = sorted(k for k in palettes["light"] if k not in LANES)

def rgba_lit(color):
    r, g, b, a = (round(min(1, max(0, x)) * 255) for x in color)
    return f"rgba(0x{r:02x}{g:02x}{b:02x}{a:02x})"

lines = [
    "// Generado por native/tools/gen-theme.py (tema «Señal»).",
    "// No editar a mano: cambiar el generador y volver a correrlo.",
    "",
    "use gpui_kit::{Rgba, rgba};",
    "",
    "/// Tokens de color de Mergeon (Señal) que el tema de gpui-kit no cubre.",
    "///",
    "/// Los nombres de Señal (`bg_*`, `text_*`, `border_*`, `sel_*`, `diff_*`,",
    "/// `lanes`) conviven con los alias heredados de shadcn (`card`, `muted_foreground`,",
    "/// `accent` = hover...). El naranja señal es la familia `primary`: relleno",
    "/// (`primary`, siempre con `primary_foreground`, tinta, encima) y texto",
    "/// (`primary_text`).",
    "#[derive(Debug, Clone, Copy)]",
    "pub struct Palette {",
]
lines += [f"    pub {name}: Rgba," for name in names]
lines += [
    "    /// Los 8 carriles del grafo (misma L en OKLCH). El 0 (señal) es el de HEAD.",
    "    pub lanes: [Rgba; 8],",
    "}",
    "",
]
for mode in ("light", "dark"):
    lines.append(f"pub const {mode.upper()}: fn() -> Palette = || Palette {{")
    for name in names:
        lines.append(f"    {name}: {rgba_lit(palettes[mode][name])},")
    lines.append("    lanes: [")
    for lane in LANES:
        lines.append(f"        {rgba_lit(palettes[mode][lane])},")
    lines.append("    ],")
    lines += ["};", ""]
with open(os.path.join(root, "crates/mergeon-ui/src/theme/palette.rs"), "w") as f:
    f.write("\n".join(lines))
for mode in ("light", "dark"):
    print(mode, {k: hexa(palettes[mode][k]) for k in ("background", "foreground", "card", "primary", "border", "muted_foreground")})
