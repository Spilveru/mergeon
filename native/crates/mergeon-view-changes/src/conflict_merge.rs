//! La parte del resolvedor de conflictos que no pinta: partir las tres
//! versiones en líneas, emparejarlas (diff de líneas), fusionarlas en tramos
//! estables, cambiados por un lado o en conflicto, y componer el resultado
//! según lo que se elige en cada conflicto.
//!
//! Todo es puro y sin GPUI más allá de `SharedString`: se ejecuta en un hilo de
//! fondo ([`analyze`], [`resync`], [`finish_result`]) y se prueba sin ventana.
//!
//! El diff es el de "patience" (anclas en las líneas únicas a los dos lados)
//! con Myers para lo que queda entre anclas, con un tope de distancia: si dos
//! trozos no se parecen en nada se toman como un reemplazo entero en vez de
//! buscar el mejor emparejamiento a cualquier coste.

use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::SharedString;
use mergeon_git_query::{ConflictKind, ConflictVersions};

/// Más allá de esta distancia de edición, Myers se rinde y el trozo queda como
/// reemplazo entero (coste acotado en archivos de cientos de miles de líneas).
const MAX_EDIT_DISTANCE: usize = 2000;
/// Profundidad máxima de anclas anidadas antes de pasar a Myers.
const MAX_ANCHOR_DEPTH: usize = 48;

/// Rótulos de los marcadores que el resolvedor pone en los conflictos sin
/// decidir del resultado.
pub const CURRENT_MARKER: &str = "<<<<<<< Current (ours)";
pub const SEPARATOR_MARKER: &str = "=======";
pub const INCOMING_MARKER: &str = ">>>>>>> Incoming (theirs)";

// ---------------------------------------------------------------------------
// Líneas
// ---------------------------------------------------------------------------

/// Final de línea dominante de un archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
        }
    }
}

/// Un texto partido en líneas sin su final.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitText {
    pub lines: Vec<String>,
    /// El texto acaba en salto de línea.
    pub final_newline: bool,
    /// Líneas que acababan en `\r\n` y en `\n` solo.
    crlf: usize,
    lf: usize,
}

impl SplitText {
    pub fn ending(&self) -> LineEnding {
        if self.crlf > self.lf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }
}

/// Parte `text` en líneas. Un `\r` antes del `\n` es parte del final, no de la
/// línea: así un lado en CRLF y otro en LF no difieren en todas sus líneas.
pub fn split_lines(text: &str) -> SplitText {
    let mut split = SplitText::default();
    if text.is_empty() {
        return split;
    }
    split.final_newline = text.ends_with('\n');
    let mut parts: Vec<&str> = text.split('\n').collect();
    if split.final_newline {
        parts.pop();
    }
    let last = parts.len().saturating_sub(1);
    for (index, part) in parts.into_iter().enumerate() {
        let terminated = index < last || split.final_newline;
        let line = match part.strip_suffix('\r') {
            Some(line) if terminated => {
                split.crlf += 1;
                line
            }
            _ => {
                if terminated {
                    split.lf += 1;
                }
                part
            }
        };
        split.lines.push(line.to_string());
    }
    split
}

/// Una línea que parece un marcador de conflicto que se quedó en el resultado.
pub fn is_marker_line(line: &str) -> bool {
    line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>")
}

/// Ids de línea compartidos por varios textos: dos líneas iguales tienen el
/// mismo id, y comparar ids es comparar enteros.
#[derive(Default)]
struct Interner<'a> {
    ids: HashMap<&'a str, u32>,
}

impl<'a> Interner<'a> {
    fn intern<S: AsRef<str>>(&mut self, lines: &'a [S]) -> Vec<u32> {
        lines
            .iter()
            .map(|line| {
                let next = self.ids.len() as u32;
                *self.ids.entry(line.as_ref()).or_insert(next)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Diff de líneas
// ---------------------------------------------------------------------------

/// Pares `(i, j)` de líneas iguales emparejadas de `a` y `b`, crecientes en
/// los dos índices.
pub(crate) fn diff_pairs(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    diff_region(a, b, 0, 0, &mut pairs, 0);
    pairs
}

fn diff_region(
    a: &[u32],
    b: &[u32],
    a_offset: usize,
    b_offset: usize,
    pairs: &mut Vec<(usize, usize)>,
    depth: usize,
) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    pairs.extend((0..prefix).map(|k| (a_offset + k, b_offset + k)));
    let (a, b) = (&a[prefix..], &b[prefix..]);
    let (a_offset, b_offset) = (a_offset + prefix, b_offset + prefix);
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);

    if !a_mid.is_empty() && !b_mid.is_empty() {
        let anchors = if depth < MAX_ANCHOR_DEPTH {
            unique_anchors(a_mid, b_mid)
        } else {
            Vec::new()
        };
        if anchors.is_empty() {
            if let Some(found) = myers(a_mid, b_mid, MAX_EDIT_DISTANCE) {
                pairs.extend(found.into_iter().map(|(i, j)| (a_offset + i, b_offset + j)));
            }
        } else {
            let (mut a_from, mut b_from) = (0, 0);
            for (i, j) in anchors {
                diff_region(
                    &a_mid[a_from..i],
                    &b_mid[b_from..j],
                    a_offset + a_from,
                    b_offset + b_from,
                    pairs,
                    depth + 1,
                );
                pairs.push((a_offset + i, b_offset + j));
                (a_from, b_from) = (i + 1, j + 1);
            }
            diff_region(
                &a_mid[a_from..],
                &b_mid[b_from..],
                a_offset + a_from,
                b_offset + b_from,
                pairs,
                depth + 1,
            );
        }
    }

    let (a_end, b_end) = (a_offset + a_mid.len(), b_offset + b_mid.len());
    pairs.extend((0..suffix).map(|k| (a_end + k, b_end + k)));
}

/// Las líneas que aparecen una sola vez en `a` y una sola vez en `b`, en la
/// subsecuencia creciente más larga: las anclas de patience diff.
fn unique_anchors(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    // id → (veces en a, posición en a, veces en b, posición en b)
    let mut seen: HashMap<u32, (u32, usize, u32, usize)> = HashMap::new();
    for (i, &line) in a.iter().enumerate() {
        let entry = seen.entry(line).or_insert((0, i, 0, 0));
        entry.0 += 1;
        entry.1 = i;
    }
    for (j, &line) in b.iter().enumerate() {
        if let Some(entry) = seen.get_mut(&line) {
            entry.2 += 1;
            entry.3 = j;
        }
    }
    let mut candidates: Vec<(usize, usize)> = seen
        .into_values()
        .filter(|&(in_a, _, in_b, _)| in_a == 1 && in_b == 1)
        .map(|(_, i, _, j)| (i, j))
        .collect();
    candidates.sort_unstable();
    longest_increasing(&candidates)
}

/// Subsecuencia más larga de `pairs` (ordenados por `.0`) creciente en `.1`.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; pairs.len()];
    for (index, &(_, j)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&tail| pairs[tail].1 < j);
        if at > 0 {
            previous[index] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(index);
        } else {
            tails[at] = index;
        }
    }
    let mut result = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(index) = cursor {
        result.push(pairs[index]);
        cursor = previous[index];
    }
    result.reverse();
    result
}

/// Myers O(ND) con el rastro de cada paso para reconstruir el camino. `None`
/// si la distancia pasa de `max_distance`.
fn myers(a: &[u32], b: &[u32], max_distance: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (max_distance as isize).min(n + m);
    let offset = max_d + 1;
    let mut v = vec![0isize; (2 * max_d + 3) as usize];
    // `trace[d]` guarda x para k = -d, -d+2, …, d tras el paso d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=max_d {
        let mut step = Vec::with_capacity(d as usize + 1);
        let mut k = -d;
        while k <= d {
            let index = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[index - 1] < v[index + 1]) {
                v[index + 1]
            } else {
                v[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[index] = x;
            step.push(x);
            if x >= n && y >= m {
                trace.push(step);
                return Some(myers_path(&trace, n, m));
            }
            k += 2;
        }
        trace.push(step);
    }
    None
}

fn myers_path(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<(usize, usize)> {
    let (mut x, mut y) = (n, m);
    let mut pairs = Vec::new();
    for d in (0..trace.len() as isize).rev() {
        if d == 0 {
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                pairs.push((x as usize, y as usize));
            }
            break;
        }
        let k = x - y;
        let previous = &trace[(d - 1) as usize];
        let at = |k: isize| previous[((k + d - 1) / 2) as usize];
        let previous_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let previous_x = at(previous_k);
        let previous_y = previous_x - previous_k;
        let (middle_x, middle_y) = if previous_k == k + 1 {
            (previous_x, previous_y + 1)
        } else {
            (previous_x + 1, previous_y)
        };
        while x > middle_x && y > middle_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        (x, y) = (previous_x, previous_y);
    }
    pairs.reverse();
    pairs
}

// ---------------------------------------------------------------------------
// Fusión
// ---------------------------------------------------------------------------

/// Qué pasó en un tramo respecto de la base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// Igual en las tres versiones.
    Stable,
    /// Solo lo cambió el lado actual: se queda ese cambio.
    Ours,
    /// Solo lo cambió lo que llega.
    Theirs,
    /// Los dos lo cambiaron igual.
    Both,
    /// Los dos lo cambiaron distinto: hay que decidir.
    Conflict,
}

/// Un tramo de la fusión: los rangos de líneas que ocupa en cada versión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub base: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
}

fn matches_of(pairs: &[(usize, usize)], len: usize) -> Vec<Option<usize>> {
    let mut map = vec![None; len];
    for &(i, j) in pairs {
        map[i] = Some(j);
    }
    map
}

/// Fusión de tres vías (el algoritmo de `diff3`): los tramos donde base,
/// actual y entrante coinciden son estables; entre ellos, lo que cambió un
/// solo lado se queda con ese lado y lo que cambiaron los dos, distinto, es un
/// conflicto. Como `zdiff3`, las líneas iguales al principio y al final de los
/// dos lados de un conflicto salen de él.
pub(crate) fn merge3(base: &[u32], ours: &[u32], theirs: &[u32]) -> Vec<Chunk> {
    let to_ours = matches_of(&diff_pairs(base, ours), base.len());
    let to_theirs = matches_of(&diff_pairs(base, theirs), base.len());
    let (nb, no, nt) = (base.len(), ours.len(), theirs.len());
    let (mut i, mut o, mut t) = (0, 0, 0);
    let mut chunks = Vec::new();
    loop {
        let (i0, o0, t0) = (i, o, t);
        while i < nb && to_ours[i] == Some(o) && to_theirs[i] == Some(t) {
            (i, o, t) = (i + 1, o + 1, t + 1);
        }
        if i > i0 {
            chunks.push(Chunk {
                kind: ChunkKind::Stable,
                base: i0..i,
                ours: o0..o,
                theirs: t0..t,
            });
        }
        if i >= nb && o >= no && t >= nt {
            break;
        }
        let mut j = i;
        while j < nb && (to_ours[j].is_none() || to_theirs[j].is_none()) {
            j += 1;
        }
        let (o_end, t_end) = if j < nb {
            (to_ours[j].unwrap_or(no), to_theirs[j].unwrap_or(nt))
        } else {
            (no, nt)
        };
        push_unstable(&mut chunks, base, ours, theirs, i..j, o..o_end, t..t_end);
        (i, o, t) = (j, o_end, t_end);
    }
    chunks
}

fn push_unstable(
    chunks: &mut Vec<Chunk>,
    base: &[u32],
    ours: &[u32],
    theirs: &[u32],
    b: Range<usize>,
    o: Range<usize>,
    t: Range<usize>,
) {
    let (base_part, ours_part, theirs_part) =
        (&base[b.clone()], &ours[o.clone()], &theirs[t.clone()]);
    let kind = match (ours_part == base_part, theirs_part == base_part) {
        (true, true) => ChunkKind::Stable,
        (true, false) => ChunkKind::Theirs,
        (false, true) => ChunkKind::Ours,
        (false, false) if ours_part == theirs_part => ChunkKind::Both,
        (false, false) => ChunkKind::Conflict,
    };
    if kind != ChunkKind::Conflict {
        chunks.push(Chunk {
            kind,
            base: b,
            ours: o,
            theirs: t,
        });
        return;
    }
    // zdiff3: lo común a los dos lados al principio y al final no es parte
    // del conflicto.
    let prefix = ours_part
        .iter()
        .zip(theirs_part)
        .take_while(|(x, y)| x == y)
        .count();
    let suffix = ours_part[prefix..]
        .iter()
        .rev()
        .zip(theirs_part[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    if prefix > 0 {
        chunks.push(Chunk {
            kind: ChunkKind::Both,
            base: b.start..b.start,
            ours: o.start..o.start + prefix,
            theirs: t.start..t.start + prefix,
        });
    }
    let ours_middle = o.start + prefix..o.end - suffix;
    let theirs_middle = t.start + prefix..t.end - suffix;
    let middle_kind = if ours[ours_middle.clone()] == theirs[theirs_middle.clone()] {
        ChunkKind::Both
    } else {
        ChunkKind::Conflict
    };
    chunks.push(Chunk {
        kind: middle_kind,
        base: b.clone(),
        ours: ours_middle,
        theirs: theirs_middle,
    });
    if suffix > 0 {
        chunks.push(Chunk {
            kind: ChunkKind::Both,
            base: b.end..b.end,
            ours: o.end - suffix..o.end,
            theirs: t.end - suffix..t.end,
        });
    }
}

/// Fusión sin base (añadido por los dos): lo igual es estable y cualquier
/// diferencia es un conflicto, porque no hay ancestro que diga quién cambió.
pub(crate) fn merge2(ours: &[u32], theirs: &[u32]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let (mut o, mut t) = (0, 0);
    let mut stable: Option<(usize, usize, usize)> = None;
    let flush = |chunks: &mut Vec<Chunk>, stable: &mut Option<(usize, usize, usize)>| {
        if let Some((o0, t0, len)) = stable.take() {
            chunks.push(Chunk {
                kind: ChunkKind::Stable,
                base: 0..0,
                ours: o0..o0 + len,
                theirs: t0..t0 + len,
            });
        }
    };
    for (i, j) in diff_pairs(ours, theirs) {
        if i > o || j > t {
            flush(&mut chunks, &mut stable);
            chunks.push(Chunk {
                kind: ChunkKind::Conflict,
                base: 0..0,
                ours: o..i,
                theirs: t..j,
            });
        }
        match &mut stable {
            Some((_, _, len)) => *len += 1,
            None => stable = Some((i, j, 1)),
        }
        (o, t) = (i + 1, j + 1);
    }
    flush(&mut chunks, &mut stable);
    if o < ours.len() || t < theirs.len() {
        chunks.push(Chunk {
            kind: ChunkKind::Conflict,
            base: 0..0,
            ours: o..ours.len(),
            theirs: t..theirs.len(),
        });
    }
    chunks
}

// ---------------------------------------------------------------------------
// Documento
// ---------------------------------------------------------------------------

/// Uno de los paneles de versión.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Base,
    Ours,
    Theirs,
}

/// Cómo se pinta una línea de un panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineTone {
    /// Igual en las tres versiones.
    Plain,
    /// Cambió sin conflicto (entra sola en el resultado, o salió de él).
    Changed,
    /// Parte del conflicto `conflict`; `offset` es su posición en el lado.
    Conflict { conflict: usize, offset: usize },
}

/// Una fila de un panel (todas miden lo mismo: lista virtual uniforme).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRow {
    /// Cabecera del conflicto número `conflict` (con su casilla de bloque).
    Header { conflict: usize },
    /// La línea `line` (desde 0) de la versión.
    Line { line: usize, tone: LineTone },
}

/// Una versión lista para pintar: sus líneas y sus filas.
#[derive(Debug, Clone, Default)]
pub struct PaneData {
    pub lines: Vec<SharedString>,
    pub rows: Vec<PaneRow>,
    /// Fila de la cabecera de cada conflicto.
    pub conflict_rows: Vec<usize>,
    /// Fila de la línea más larga (la que mide la lista para el scroll
    /// horizontal).
    pub widest_row: usize,
    /// La versión existe (un lado borrado no tiene panel).
    pub present: bool,
}

/// Qué se le enseña al usuario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    /// Texto con los dos lados: tres paneles y resultado editable.
    Text,
    /// Se elige una versión entera (o borrar): binarios y archivos borrados
    /// por un lado.
    Whole,
}

/// El análisis de un archivo en conflicto. Se calcula en un hilo de fondo y
/// no cambia: lo que decide el usuario vive aparte ([`Pick`]).
#[derive(Debug, Clone)]
pub struct MergeDocument {
    pub path: String,
    pub kind: DocumentKind,
    pub conflict_kind: ConflictKind,
    pub binary: bool,
    /// Los conflictos salen de los marcadores del working tree (no había
    /// etapas en el índice).
    pub from_markers: bool,
    pub has_base: bool,
    pub line_ending: LineEnding,
    pub final_newline: bool,
    pub chunks: Vec<Chunk>,
    /// Tramo de cada conflicto.
    pub conflicts: Vec<usize>,
    pub base: PaneData,
    pub ours: PaneData,
    pub theirs: PaneData,
    /// Tamaño en bytes de cada versión (base, actual, entrante).
    pub sizes: [Option<u64>; 3],
}

impl MergeDocument {
    pub fn pane(&self, pane: Pane) -> &PaneData {
        match pane {
            Pane::Base => &self.base,
            Pane::Ours => &self.ours,
            Pane::Theirs => &self.theirs,
        }
    }

    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// El tramo del conflicto `conflict`.
    pub fn conflict_chunk(&self, conflict: usize) -> Option<&Chunk> {
        self.conflicts
            .get(conflict)
            .map(|&chunk| &self.chunks[chunk])
    }

    /// Cuántas líneas tiene `pane` en el conflicto `conflict`.
    pub fn side_len(&self, conflict: usize, pane: Pane) -> usize {
        self.conflict_chunk(conflict).map_or(0, |chunk| match pane {
            Pane::Base => chunk.base.len(),
            Pane::Ours => chunk.ours.len(),
            Pane::Theirs => chunk.theirs.len(),
        })
    }

    fn push_lines(text: &mut String, lines: &[SharedString]) {
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
    }

    /// El texto con el que entra el tramo `chunk` en el resultado. Cada línea
    /// lleva su `\n` (el final del archivo se ajusta al guardar).
    pub fn chunk_text(&self, chunk: usize, pick: &Pick) -> String {
        let Some(part) = self.chunks.get(chunk) else {
            return String::new();
        };
        let ours = &self.ours.lines[part.ours.clone()];
        let theirs = &self.theirs.lines[part.theirs.clone()];
        let mut text = String::new();
        match part.kind {
            ChunkKind::Stable | ChunkKind::Ours | ChunkKind::Both => {
                Self::push_lines(&mut text, ours);
            }
            ChunkKind::Theirs => Self::push_lines(&mut text, theirs),
            ChunkKind::Conflict => match pick {
                Pick::Unresolved => {
                    text.push_str(CURRENT_MARKER);
                    text.push('\n');
                    Self::push_lines(&mut text, ours);
                    text.push_str(SEPARATOR_MARKER);
                    text.push('\n');
                    Self::push_lines(&mut text, theirs);
                    text.push_str(INCOMING_MARKER);
                    text.push('\n');
                }
                Pick::Base => Self::push_lines(&mut text, &self.base.lines[part.base.clone()]),
                Pick::Lines {
                    ours: ours_picked,
                    theirs: theirs_picked,
                    theirs_first,
                } => {
                    let picked = |lines: &[SharedString], picked: &[bool], text: &mut String| {
                        for (line, _) in lines
                            .iter()
                            .zip(picked.iter().chain(std::iter::repeat(&false)))
                            .filter(|(_, picked)| **picked)
                        {
                            text.push_str(line);
                            text.push('\n');
                        }
                    };
                    if *theirs_first {
                        picked(theirs, theirs_picked, &mut text);
                        picked(ours, ours_picked, &mut text);
                    } else {
                        picked(ours, ours_picked, &mut text);
                        picked(theirs, theirs_picked, &mut text);
                    }
                }
            },
        }
        text
    }

    /// El resultado inicial (todo lo que no es conflicto ya fusionado; los
    /// conflictos, con marcadores) y la línea donde empieza cada tramo. El
    /// último valor es el número de líneas del texto contando la vacía tras
    /// el último `\n` (lo que cuenta un `Rope`).
    pub fn initial_result(&self) -> (String, Vec<usize>) {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(self.chunks.len() + 1);
        let mut line = 0;
        for chunk in 0..self.chunks.len() {
            starts.push(line);
            let part = self.chunk_text(chunk, &Pick::Unresolved);
            line += part.matches('\n').count();
            text.push_str(&part);
        }
        starts.push(line + 1);
        (text, starts)
    }
}

/// Lo decidido en un conflicto.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pick {
    /// Sin decidir: en el resultado quedan los marcadores.
    #[default]
    Unresolved,
    /// Las líneas marcadas de cada lado, en el orden indicado.
    Lines {
        ours: Vec<bool>,
        theirs: Vec<bool>,
        theirs_first: bool,
    },
    /// La versión de la base.
    Base,
}

/// Los botones de un conflicto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictChoice {
    /// "Take current": lo que hay en HEAD.
    Current,
    /// "Take incoming": lo que llega.
    Incoming,
    /// "Take both (current first)".
    BothCurrentFirst,
    /// "Take both (incoming first)".
    BothIncomingFirst,
    /// "Take base": el ancestro común.
    Base,
}

impl ConflictChoice {
    pub const ALL: [ConflictChoice; 5] = [
        Self::Current,
        Self::Incoming,
        Self::BothCurrentFirst,
        Self::BothIncomingFirst,
        Self::Base,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Take current",
            Self::Incoming => "Take incoming",
            Self::BothCurrentFirst => "Take both (current first)",
            Self::BothIncomingFirst => "Take both (incoming first)",
            Self::Base => "Take base",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Current => "take-current",
            Self::Incoming => "take-incoming",
            Self::BothCurrentFirst => "take-both-current-first",
            Self::BothIncomingFirst => "take-both-incoming-first",
            Self::Base => "take-base",
        }
    }

    /// Lo que deja decidido en un conflicto con `ours` y `theirs` líneas.
    pub fn pick(self, ours: usize, theirs: usize) -> Pick {
        let lines = |take_ours: bool, take_theirs: bool, theirs_first: bool| Pick::Lines {
            ours: vec![take_ours; ours],
            theirs: vec![take_theirs; theirs],
            theirs_first,
        };
        match self {
            Self::Current => lines(true, false, false),
            Self::Incoming => lines(false, true, false),
            Self::BothCurrentFirst => lines(true, true, false),
            Self::BothIncomingFirst => lines(true, true, true),
            Self::Base => Pick::Base,
        }
    }
}

impl Pick {
    /// La línea `offset` de `pane` entra en el resultado.
    pub fn includes(&self, pane: Pane, offset: usize) -> bool {
        match (self, pane) {
            (Pick::Lines { ours, .. }, Pane::Ours) => ours.get(offset).copied().unwrap_or(false),
            (Pick::Lines { theirs, .. }, Pane::Theirs) => {
                theirs.get(offset).copied().unwrap_or(false)
            }
            (Pick::Base, Pane::Base) => true,
            _ => false,
        }
    }

    /// Estado de la casilla de bloque de `pane`: (marcada, a medias).
    pub fn side_state(&self, pane: Pane, len: usize) -> (bool, bool) {
        let picked = (0..len)
            .filter(|&offset| self.includes(pane, offset))
            .count();
        (len > 0 && picked == len, picked > 0 && picked < len)
    }

    fn lines_or_empty(self, ours: usize, theirs: usize) -> (Vec<bool>, Vec<bool>, bool) {
        match self {
            Pick::Lines {
                ours: o,
                theirs: t,
                theirs_first,
            } => (o, t, theirs_first),
            _ => (vec![false; ours], vec![false; theirs], false),
        }
    }

    /// Marca o desmarca la línea `offset` de `pane`.
    pub fn toggle_line(self, pane: Pane, offset: usize, ours: usize, theirs: usize) -> Pick {
        let (mut o, mut t, theirs_first) = self.lines_or_empty(ours, theirs);
        let side = match pane {
            Pane::Ours => &mut o,
            Pane::Theirs => &mut t,
            Pane::Base => return Pick::Base,
        };
        if let Some(value) = side.get_mut(offset) {
            *value = !*value;
        }
        Pick::Lines {
            ours: o,
            theirs: t,
            theirs_first,
        }
    }

    /// La casilla de bloque de `pane`: si estaba entera, la quita; si no, la
    /// pone entera.
    pub fn toggle_side(self, pane: Pane, ours: usize, theirs: usize) -> Pick {
        let len = if pane == Pane::Ours { ours } else { theirs };
        let (checked, _) = self.side_state(pane, len);
        let (mut o, mut t, theirs_first) = self.lines_or_empty(ours, theirs);
        match pane {
            Pane::Ours => o = vec![!checked; ours],
            Pane::Theirs => t = vec![!checked; theirs],
            Pane::Base => return Pick::Base,
        }
        Pick::Lines {
            ours: o,
            theirs: t,
            theirs_first,
        }
    }
}

fn pane_data(lines: &[String], present: bool) -> PaneData {
    PaneData {
        lines: lines
            .iter()
            .map(|line| SharedString::from(line.clone()))
            .collect(),
        present,
        ..PaneData::default()
    }
}

/// Las filas de los tres paneles a partir de los tramos.
fn build_rows(document: &mut MergeDocument) {
    let mut conflict = 0;
    for chunk in &document.chunks {
        for pane in [Pane::Base, Pane::Ours, Pane::Theirs] {
            let (range, changed) = match pane {
                Pane::Base => (chunk.base.clone(), chunk.kind != ChunkKind::Stable),
                Pane::Ours => (
                    chunk.ours.clone(),
                    matches!(chunk.kind, ChunkKind::Ours | ChunkKind::Both),
                ),
                Pane::Theirs => (
                    chunk.theirs.clone(),
                    matches!(chunk.kind, ChunkKind::Theirs | ChunkKind::Both),
                ),
            };
            let data = match pane {
                Pane::Base => &mut document.base,
                Pane::Ours => &mut document.ours,
                Pane::Theirs => &mut document.theirs,
            };
            if chunk.kind == ChunkKind::Conflict {
                data.conflict_rows.push(data.rows.len());
                data.rows.push(PaneRow::Header { conflict });
            }
            for (offset, line) in range.enumerate() {
                let tone = if chunk.kind == ChunkKind::Conflict {
                    LineTone::Conflict { conflict, offset }
                } else if changed {
                    LineTone::Changed
                } else {
                    LineTone::Plain
                };
                data.rows.push(PaneRow::Line { line, tone });
            }
        }
        if chunk.kind == ChunkKind::Conflict {
            conflict += 1;
        }
    }
    for data in [&mut document.base, &mut document.ours, &mut document.theirs] {
        data.widest_row = data
            .rows
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| match row {
                PaneRow::Line { line, .. } => data.lines[*line].len(),
                PaneRow::Header { .. } => 0,
            })
            .map_or(0, |(index, _)| index);
    }
}

/// Un archivo sin etapas en el índice: los conflictos salen de sus
/// marcadores (`<<<<<<<` / `|||||||` / `=======` / `>>>>>>>`). Fuera de un
/// bloque, una línea que empiece por `=======`, `|||||||` o `>>>>>>>` es texto
/// normal (un subrayado de reStructuredText, por ejemplo). Un bloque que no se
/// cierra se queda como texto.
pub fn document_from_markers(path: &str, text: &str) -> MergeDocument {
    #[derive(PartialEq, Clone, Copy)]
    enum Section {
        Current,
        Base,
        Incoming,
    }
    let split = split_lines(text);
    let (mut base, mut ours, mut theirs) = (Vec::new(), Vec::new(), Vec::new());
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut has_base = false;
    let mut section: Option<Section> = None;
    // Bloque abierto: sus líneas originales (por si no se cierra) y sus lados.
    let mut raw: Vec<&str> = Vec::new();
    let (mut block_ours, mut block_base, mut block_theirs) =
        (Vec::<&str>::new(), Vec::<&str>::new(), Vec::<&str>::new());

    let push_stable = |line: &str,
                       base: &mut Vec<String>,
                       ours: &mut Vec<String>,
                       theirs: &mut Vec<String>,
                       chunks: &mut Vec<Chunk>| {
        match chunks.last_mut() {
            Some(chunk) if chunk.kind == ChunkKind::Stable => {
                chunk.base.end += 1;
                chunk.ours.end += 1;
                chunk.theirs.end += 1;
            }
            _ => chunks.push(Chunk {
                kind: ChunkKind::Stable,
                base: base.len()..base.len() + 1,
                ours: ours.len()..ours.len() + 1,
                theirs: theirs.len()..theirs.len() + 1,
            }),
        }
        base.push(line.to_string());
        ours.push(line.to_string());
        theirs.push(line.to_string());
    };

    for line in &split.lines {
        let line = line.as_str();
        if section.is_none() {
            if line.starts_with("<<<<<<<") {
                section = Some(Section::Current);
                raw = vec![line];
                block_ours.clear();
                block_base.clear();
                block_theirs.clear();
            } else {
                push_stable(line, &mut base, &mut ours, &mut theirs, &mut chunks);
            }
            continue;
        }
        raw.push(line);
        let open = section.unwrap_or(Section::Current);
        if open == Section::Current && line.starts_with("|||||||") {
            section = Some(Section::Base);
            has_base = true;
        } else if open != Section::Incoming && line.starts_with("=======") {
            section = Some(Section::Incoming);
        } else if open == Section::Incoming && line.starts_with(">>>>>>>") {
            section = None;
            chunks.push(Chunk {
                kind: ChunkKind::Conflict,
                base: base.len()..base.len() + block_base.len(),
                ours: ours.len()..ours.len() + block_ours.len(),
                theirs: theirs.len()..theirs.len() + block_theirs.len(),
            });
            base.extend(block_base.iter().map(|line| line.to_string()));
            ours.extend(block_ours.iter().map(|line| line.to_string()));
            theirs.extend(block_theirs.iter().map(|line| line.to_string()));
        } else {
            match open {
                Section::Current => block_ours.push(line),
                Section::Base => block_base.push(line),
                Section::Incoming => block_theirs.push(line),
            }
        }
    }
    if section.is_some() {
        for line in raw {
            push_stable(line, &mut base, &mut ours, &mut theirs, &mut chunks);
        }
    }

    let mut document = MergeDocument {
        path: path.to_string(),
        kind: DocumentKind::Text,
        conflict_kind: ConflictKind::Unstaged,
        binary: false,
        from_markers: true,
        has_base,
        line_ending: split.ending(),
        final_newline: split.final_newline,
        conflicts: conflict_indices(&chunks),
        chunks,
        base: pane_data(&base, has_base),
        ours: pane_data(&ours, true),
        theirs: pane_data(&theirs, true),
        sizes: [None, Some(text.len() as u64), None],
    };
    build_rows(&mut document);
    document
}

fn conflict_indices(chunks: &[Chunk]) -> Vec<usize> {
    chunks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| chunk.kind == ChunkKind::Conflict)
        .map(|(index, _)| index)
        .collect()
}

/// Analiza las versiones de un archivo en conflicto (en un hilo de fondo: es
/// lineal en el tamaño, más el diff).
pub fn analyze(versions: &ConflictVersions) -> MergeDocument {
    let conflict_kind = versions.kind();
    if conflict_kind == ConflictKind::Unstaged {
        return document_from_markers(&versions.path, versions.working.as_deref().unwrap_or(""));
    }
    let text_of = |stage: &Option<mergeon_git_query::ConflictStage>| {
        stage
            .as_ref()
            .filter(|stage| !stage.binary)
            .map(|stage| split_lines(&stage.text))
    };
    let base = text_of(&versions.base);
    let ours = text_of(&versions.ours);
    let theirs = text_of(&versions.theirs);
    let binary = versions.is_binary();
    let sizes = [
        versions.base.as_ref().map(|stage| stage.size),
        versions.ours.as_ref().map(|stage| stage.size),
        versions.theirs.as_ref().map(|stage| stage.size),
    ];
    let reference = ours.as_ref().or(theirs.as_ref()).or(base.as_ref());
    let line_ending = reference.map(SplitText::ending).unwrap_or_default();

    let whole = binary || versions.ours.is_none() || versions.theirs.is_none();
    if whole {
        // Sin fusión: cada versión que haya, entera y sin tonos.
        let empty = SplitText::default();
        let lines = |split: &Option<SplitText>| split.as_ref().unwrap_or(&empty).lines.clone();
        let (b, o, t) = (lines(&base), lines(&ours), lines(&theirs));
        let plain = |data: &mut PaneData| {
            data.rows = (0..data.lines.len())
                .map(|line| PaneRow::Line {
                    line,
                    tone: LineTone::Plain,
                })
                .collect();
        };
        let mut document = MergeDocument {
            path: versions.path.clone(),
            kind: DocumentKind::Whole,
            conflict_kind,
            binary,
            from_markers: false,
            has_base: versions.base.is_some(),
            line_ending,
            final_newline: reference.is_some_and(|split| split.final_newline),
            chunks: Vec::new(),
            conflicts: Vec::new(),
            base: pane_data(&b, versions.base.is_some()),
            ours: pane_data(&o, versions.ours.is_some()),
            theirs: pane_data(&t, versions.theirs.is_some()),
            sizes,
        };
        plain(&mut document.base);
        plain(&mut document.ours);
        plain(&mut document.theirs);
        return document;
    }

    let ours = ours.unwrap_or_default();
    let theirs = theirs.unwrap_or_default();
    let base_split = base.unwrap_or_default();
    let mut interner = Interner::default();
    let ours_ids = interner.intern(&ours.lines);
    let theirs_ids = interner.intern(&theirs.lines);
    let base_ids = interner.intern(&base_split.lines);
    let has_base = versions.base.is_some();
    let chunks = if has_base && !base_split.lines.is_empty() {
        merge3(&base_ids, &ours_ids, &theirs_ids)
    } else {
        merge2(&ours_ids, &theirs_ids)
    };
    // El salto de línea final: el de los dos si coinciden; si no, el del lado
    // que lo cambió respecto de la base.
    let final_newline = if ours.final_newline == theirs.final_newline
        || (has_base && base_split.final_newline != ours.final_newline)
    {
        ours.final_newline
    } else {
        theirs.final_newline
    };

    let mut document = MergeDocument {
        path: versions.path.clone(),
        kind: DocumentKind::Text,
        conflict_kind,
        binary: false,
        from_markers: false,
        has_base,
        line_ending,
        final_newline,
        conflicts: conflict_indices(&chunks),
        chunks,
        base: pane_data(&base_split.lines, has_base),
        ours: pane_data(&ours.lines, true),
        theirs: pane_data(&theirs.lines, true),
        sizes,
    };
    build_rows(&mut document);
    document
}

// ---------------------------------------------------------------------------
// Resultado editado a mano
// ---------------------------------------------------------------------------

/// Cómo quedaron los tramos del resultado después de que el usuario lo
/// editara.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resync {
    /// Línea de inicio de cada tramo en el texto nuevo (y el total al final).
    pub starts: Vec<usize>,
    /// Tramos cuyo texto cambió.
    pub touched: Vec<bool>,
    /// De los tocados, los que todavía llevan marcadores.
    pub markers: Vec<bool>,
}

/// Lleva los límites de los tramos (`starts`, en líneas de `old`) a `new`
/// emparejando las líneas de los dos textos. Lo que se escribe entre dos
/// tramos se queda en el de arriba; lo que reemplaza el principio de un tramo,
/// en ese tramo.
pub fn resync(old: &str, new: &str, starts: &[usize]) -> Resync {
    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();
    let mut interner = Interner::default();
    let old_ids = interner.intern(&old_lines);
    let new_ids = interner.intern(&new_lines);
    let map = matches_of(&diff_pairs(&old_ids, &new_ids), old_lines.len());

    let map_boundary = |p: usize| -> usize {
        if p >= old_lines.len() {
            return new_lines.len();
        }
        if let Some(q) = map[p] {
            return q;
        }
        // `p` está en un trozo cambiado [a0, a1) → [b0, b1).
        let mut a0 = p;
        while a0 > 0 && map[a0 - 1].is_none() {
            a0 -= 1;
        }
        if p == a0 {
            return if a0 == 0 {
                0
            } else {
                map[a0 - 1].map_or(0, |q| q + 1)
            };
        }
        let mut a1 = p;
        while a1 < old_lines.len() && map[a1].is_none() {
            a1 += 1;
        }
        if a1 >= old_lines.len() {
            new_lines.len()
        } else {
            map[a1].unwrap_or(new_lines.len())
        }
    };

    let new_starts: Vec<usize> = starts.iter().map(|&p| map_boundary(p)).collect();
    let chunks = starts.len().saturating_sub(1);
    let mut touched = vec![false; chunks];
    let mut markers = vec![false; chunks];
    for chunk in 0..chunks {
        let (s, e) = (starts[chunk], starts[chunk + 1].min(old_lines.len()));
        let (ns, ne) = (
            new_starts[chunk],
            new_starts[chunk + 1].min(new_lines.len()),
        );
        let same = e.saturating_sub(s) == ne.saturating_sub(ns)
            && (s..e).all(|p| map[p] == Some(ns + (p - s)));
        if !same {
            touched[chunk] = true;
            markers[chunk] = new_lines[ns.min(ne)..ne]
                .iter()
                .any(|line| is_marker_line(line));
        }
    }
    Resync {
        starts: new_starts,
        touched,
        markers,
    }
}

/// Lo que se escribe al marcar como resuelto: el texto del editor con el
/// salto final del archivo original y sus finales de línea (CRLF incluido).
pub fn finish_result(text: &str, line_ending: LineEnding, final_newline: bool) -> String {
    let mut text = text.replace("\r\n", "\n");
    if !final_newline && text.ends_with('\n') {
        text.pop();
    }
    match line_ending {
        LineEnding::Lf => text,
        LineEnding::CrLf => text.replace('\n', "\r\n"),
    }
}

/// Líneas del texto que parecen marcadores de conflicto.
pub fn count_marker_lines(text: &str) -> usize {
    text.split('\n').filter(|line| is_marker_line(line)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_git_query::ConflictStage;

    fn ids(text: &str) -> Vec<u32> {
        text.chars().map(|c| c as u32).collect()
    }

    fn stage(text: &str) -> Option<ConflictStage> {
        Some(ConflictStage {
            oid: String::new(),
            mode: "100644".into(),
            size: text.len() as u64,
            binary: false,
            text: text.into(),
        })
    }

    fn versions(base: Option<&str>, ours: Option<&str>, theirs: Option<&str>) -> ConflictVersions {
        ConflictVersions {
            path: "f.txt".into(),
            base: base.and_then(stage),
            ours: ours.and_then(stage),
            theirs: theirs.and_then(stage),
            working: None,
            sides: Default::default(),
        }
    }

    fn kinds(document: &MergeDocument) -> Vec<ChunkKind> {
        document.chunks.iter().map(|chunk| chunk.kind).collect()
    }

    /// El resultado con cada conflicto decidido por `pick`.
    fn result_with(document: &MergeDocument, picks: &[Pick]) -> String {
        let mut conflict = 0;
        let mut text = String::new();
        for (index, chunk) in document.chunks.iter().enumerate() {
            let pick = if chunk.kind == ChunkKind::Conflict {
                conflict += 1;
                picks[conflict - 1].clone()
            } else {
                Pick::Unresolved
            };
            text.push_str(&document.chunk_text(index, &pick));
        }
        finish_result(&text, document.line_ending, document.final_newline)
    }

    #[test]
    fn diff_pairs_finds_the_common_lines() {
        let pairs = diff_pairs(&ids("abcabba"), &ids("cbabac"));
        // Una subsecuencia común de largo 4 (la LCS de este ejemplo clásico).
        assert_eq!(pairs.len(), 4);
        let (a, b) = (ids("abcabba"), ids("cbabac"));
        assert!(pairs.iter().all(|&(i, j)| a[i] == b[j]));
        assert!(pairs.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
    }

    #[test]
    fn myers_gives_up_past_its_distance_and_patience_anchors_on_unique_lines() {
        assert_eq!(myers(&ids("abc"), &ids("xyz"), 2), None);
        assert_eq!(
            myers(&ids("abc"), &ids("abc"), 0),
            Some(vec![(0, 0), (1, 1), (2, 2)])
        );
        assert_eq!(
            unique_anchors(&ids("xaybz"), &ids("qaybw")),
            vec![(1, 1), (2, 2), (3, 3)]
        );
    }

    #[test]
    fn a_large_file_with_a_small_edit_stays_cheap() {
        let a: Vec<u32> = (0..200_000).collect();
        let mut b = a.clone();
        b[100_000] = 9_999_999;
        b.insert(150_000, 8_888_888);
        let pairs = diff_pairs(&a, &b);
        assert_eq!(pairs.len(), 199_999);
    }

    #[test]
    fn splits_lines_and_detects_crlf() {
        let split = split_lines("a\r\nb\r\nc");
        assert_eq!(split.lines, ["a", "b", "c"]);
        assert!(!split.final_newline);
        assert_eq!(split.ending(), LineEnding::CrLf);
        let split = split_lines("a\nb\n");
        assert_eq!(split.lines, ["a", "b"]);
        assert!(split.final_newline);
        assert_eq!(split.ending(), LineEnding::Lf);
        assert_eq!(split_lines(""), SplitText::default());
    }

    #[test]
    fn merges_non_conflicting_changes_and_isolates_conflicts() {
        let document = analyze(&versions(
            Some("a\nb\nc\nk\nd\ne\n"),
            Some("A\nb\nc\nk\nd1\ne\n"),
            Some("a\nb\nC\nk\nd2\ne\n"),
        ));
        assert_eq!(document.kind, DocumentKind::Text);
        assert_eq!(document.conflict_count(), 1);
        assert!(kinds(&document).contains(&ChunkKind::Ours));
        assert!(kinds(&document).contains(&ChunkKind::Theirs));
        // Lo que cambió un solo lado entra solo.
        assert_eq!(
            result_with(&document, &[ConflictChoice::Current.pick(1, 1)]),
            "A\nb\nC\nk\nd1\ne\n"
        );
        assert_eq!(
            result_with(&document, &[ConflictChoice::Incoming.pick(1, 1)]),
            "A\nb\nC\nk\nd2\ne\n"
        );
    }

    #[test]
    fn every_block_choice_composes_its_text() {
        let document = analyze(&versions(
            Some("top\nold\nbottom\n"),
            Some("top\nmine 1\nmine 2\nbottom\n"),
            Some("top\ntheirs\nbottom\n"),
        ));
        assert_eq!(document.conflict_count(), 1);
        let pick = |choice: ConflictChoice| choice.pick(2, 1);
        assert_eq!(
            result_with(&document, &[pick(ConflictChoice::BothCurrentFirst)]),
            "top\nmine 1\nmine 2\ntheirs\nbottom\n"
        );
        assert_eq!(
            result_with(&document, &[pick(ConflictChoice::BothIncomingFirst)]),
            "top\ntheirs\nmine 1\nmine 2\nbottom\n"
        );
        assert_eq!(
            result_with(&document, &[pick(ConflictChoice::Base)]),
            "top\nold\nbottom\n"
        );
        // Sin decidir quedan los marcadores.
        let unresolved = result_with(&document, &[Pick::Unresolved]);
        assert_eq!(count_marker_lines(&unresolved), 2);
        assert!(unresolved.contains(
            "<<<<<<< Current (ours)\nmine 1\nmine 2\n=======\ntheirs\n>>>>>>> Incoming (theirs)\n"
        ));
    }

    #[test]
    fn picks_single_lines_from_each_side() {
        let document = analyze(&versions(
            Some("x\n"),
            Some("o1\no2\no3\n"),
            Some("t1\nt2\n"),
        ));
        let pick = Pick::Unresolved
            .toggle_line(Pane::Ours, 0, 3, 2)
            .toggle_line(Pane::Ours, 2, 3, 2)
            .toggle_line(Pane::Theirs, 1, 3, 2);
        assert_eq!(
            result_with(&document, std::slice::from_ref(&pick)),
            "o1\no3\nt2\n"
        );
        assert_eq!(pick.side_state(Pane::Ours, 3), (false, true));
        // La casilla de bloque completa el lado, y otra vez lo vacía.
        let pick = pick.toggle_side(Pane::Ours, 3, 2);
        assert_eq!(pick.side_state(Pane::Ours, 3), (true, false));
        assert_eq!(
            result_with(&document, std::slice::from_ref(&pick)),
            "o1\no2\no3\nt2\n"
        );
        let pick = pick.toggle_side(Pane::Ours, 3, 2);
        assert_eq!(result_with(&document, &[pick]), "t2\n");
        // Ambos con lo entrante primero, y luego una línea menos.
        let pick = ConflictChoice::BothIncomingFirst
            .pick(3, 2)
            .toggle_line(Pane::Theirs, 0, 3, 2);
        assert_eq!(result_with(&document, &[pick]), "t2\no1\no2\no3\n");
    }

    #[test]
    fn add_add_conflicts_keep_the_common_lines_stable() {
        let document = analyze(&versions(
            None,
            Some("same\nmine\nend\n"),
            Some("same\ntheirs\nend\n"),
        ));
        assert_eq!(document.conflict_kind, ConflictKind::BothAdded);
        assert!(!document.has_base);
        assert_eq!(
            kinds(&document),
            [ChunkKind::Stable, ChunkKind::Conflict, ChunkKind::Stable]
        );
        assert_eq!(
            result_with(&document, &[ConflictChoice::Incoming.pick(1, 1)]),
            "same\ntheirs\nend\n"
        );
    }

    #[test]
    fn keeps_crlf_and_the_missing_final_newline() {
        let document = analyze(&versions(
            Some("a\r\nb\r\nc"),
            Some("a\r\nB1\r\nc"),
            Some("a\nB2\nc"),
        ));
        assert_eq!(document.line_ending, LineEnding::CrLf);
        assert!(!document.final_newline);
        // Un lado en LF no hace que difieran todas las líneas.
        assert_eq!(document.conflict_count(), 1);
        assert_eq!(
            result_with(&document, &[ConflictChoice::BothCurrentFirst.pick(1, 1)]),
            "a\r\nB1\r\nB2\r\nc"
        );
    }

    #[test]
    fn delete_modify_and_binary_conflicts_choose_a_whole_version() {
        let document = analyze(&versions(Some("a\n"), None, Some("b\n")));
        assert_eq!(document.kind, DocumentKind::Whole);
        assert_eq!(document.conflict_kind, ConflictKind::DeletedByUs);
        assert!(!document.ours.present);
        assert_eq!(document.theirs.lines.len(), 1);

        let mut binary = versions(Some(""), Some(""), Some(""));
        binary.theirs.as_mut().unwrap().binary = true;
        let document = analyze(&binary);
        assert_eq!(document.kind, DocumentKind::Whole);
        assert!(document.binary);
    }

    #[test]
    fn rows_mark_headers_conflict_lines_and_changes() {
        let document = analyze(&versions(
            Some("a\nb\nc\n"),
            Some("A\nb\nc1\n"),
            Some("a\nb\nc2\n"),
        ));
        assert_eq!(document.ours.conflict_rows, [2]);
        assert_eq!(
            document.ours.rows,
            [
                PaneRow::Line {
                    line: 0,
                    tone: LineTone::Changed
                },
                PaneRow::Line {
                    line: 1,
                    tone: LineTone::Plain
                },
                PaneRow::Header { conflict: 0 },
                PaneRow::Line {
                    line: 2,
                    tone: LineTone::Conflict {
                        conflict: 0,
                        offset: 0
                    }
                },
            ]
        );
        assert_eq!(
            document.theirs.rows[0],
            PaneRow::Line {
                line: 0,
                tone: LineTone::Plain
            }
        );
    }

    #[test]
    fn the_initial_result_counts_lines_per_chunk() {
        let document = analyze(&versions(Some("a\nb\n"), Some("a\nB1\n"), Some("a\nB2\n")));
        let (text, starts) = document.initial_result();
        assert_eq!(
            text,
            "a\n<<<<<<< Current (ours)\nB1\n=======\nB2\n>>>>>>> Incoming (theirs)\n"
        );
        // El tramo estable ocupa la línea 0; el conflicto, de la 1 a la 5; y
        // la línea vacía tras el último salto cuenta como la 6.
        assert_eq!(starts, [0, 1, 7]);
    }

    #[test]
    fn resync_follows_edits_and_marks_the_touched_chunks() {
        // Tramos: [0,2) "a b", [2,4) "<<< x", [4,6) "c d", total 7.
        let old = "a\nb\n<<<<<<< x\n>>>>>>> y\nc\nd\n";
        let starts = [0, 2, 4, 7];

        // Se escribe una línea en el primer tramo.
        let new = "a\nnew\nb\n<<<<<<< x\n>>>>>>> y\nc\nd\n";
        let sync = resync(old, new, &starts);
        assert_eq!(sync.starts, [0, 3, 5, 8]);
        assert_eq!(sync.touched, [true, false, false]);

        // Se resuelve el conflicto a mano.
        let new = "a\nb\nresolved\nc\nd\n";
        let sync = resync(old, new, &starts);
        assert_eq!(sync.starts, [0, 2, 3, 6]);
        assert_eq!(sync.touched, [false, true, false]);
        assert_eq!(sync.markers, [false, false, false]);

        // Se edita dentro del conflicto pero quedan marcadores.
        let new = "a\nb\n<<<<<<< x\nmid\n>>>>>>> y\nc\nd\n";
        let sync = resync(old, new, &starts);
        assert_eq!(sync.touched, [false, true, false]);
        assert_eq!(sync.markers, [false, true, false]);

        // Sin cambios no hay nada tocado.
        let sync = resync(old, old, &starts);
        assert_eq!(sync.starts, starts);
        assert_eq!(sync.touched, [false, false, false]);
    }

    #[test]
    fn finishing_restores_endings_and_the_final_newline() {
        assert_eq!(
            finish_result("a\nb\n", LineEnding::CrLf, true),
            "a\r\nb\r\n"
        );
        assert_eq!(finish_result("a\nb\n", LineEnding::Lf, false), "a\nb");
        // Lo pegado con CRLF no se duplica.
        assert_eq!(
            finish_result("a\r\nb\n", LineEnding::CrLf, true),
            "a\r\nb\r\n"
        );
    }

    #[test]
    fn reads_marker_blocks_when_there_are_no_stages() {
        let document = document_from_markers(
            "f",
            "before\n<<<<<<< HEAD\nmine\n||||||| base\nancestor\n=======\ntheirs\n>>>>>>> x\nafter\n",
        );
        assert!(document.from_markers);
        assert!(document.has_base);
        assert_eq!(
            kinds(&document),
            [ChunkKind::Stable, ChunkKind::Conflict, ChunkKind::Stable]
        );
        assert_eq!(
            result_with(&document, &[ConflictChoice::Base.pick(1, 1)]),
            "before\nancestor\nafter\n"
        );
        assert_eq!(
            result_with(&document, &[ConflictChoice::BothCurrentFirst.pick(1, 1)]),
            "before\nmine\ntheirs\nafter\n"
        );
    }

    #[test]
    fn marker_lookalikes_outside_a_block_and_unclosed_blocks_are_text() {
        let document = document_from_markers(
            "f",
            "Title\n=======\n\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n>>>>>>> quote\n<<<<<<< open\nz\n",
        );
        assert_eq!(document.conflict_count(), 1);
        assert_eq!(
            result_with(&document, &[ConflictChoice::Current.pick(1, 1)]),
            "Title\n=======\n\na\n>>>>>>> quote\n<<<<<<< open\nz\n"
        );
    }
}
