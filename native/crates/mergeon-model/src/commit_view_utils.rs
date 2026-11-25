//! Utilidades de la vista de commits
//! (`src/components/repository/commitViewUtils.js`): colores de rama, grafo
//! (carriles y aristas), refs de cada commit, badges y pertenencia a la rama.
//!
//! Además de las funciones del archivo original se incluye la poca lógica pura
//! que tenían sus dos consumidores: `normalizeRawCommit` y el número de
//! carriles de `CommitView.vue`, y los títulos y la marca de HEAD de
//! `CommitHistoryList.vue`.
//!
//! Los colores viajan como en el original, en texto CSS (`oklch(0.68 0.12 264)`,
//! `var(--primary)`). GPUI no entiende CSS, así que [`parse_css_color`] los
//! convierte en un [`ColorSpec`] con el que la interfaz resuelve el color real.

use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::format::format_relative_date_at;
use crate::jsstr::{js_split_whitespace, js_trim};

// ---------------------------------------------------------------------------
// Colores
// ---------------------------------------------------------------------------

const MAIN_COLOR: &str = "oklch(0.72 0.12 180)";
const DEVELOP_COLOR: &str = "oklch(0.68 0.14 330)";
const PRIMARY_COLOR: &str = "var(--primary)";
const WARNING_COLOR: &str = "var(--warning)";

/// Mid-lightness oklch keeps hashed branch colors legible on both themes.
pub fn get_branch_color(branch: &str) -> String {
    if branch.is_empty() {
        return PRIMARY_COLOR.to_string();
    }

    match branch {
        "main" | "master" => MAIN_COLOR.to_string(),
        "develop" | "dev" => DEVELOP_COLOR.to_string(),
        _ => {
            // `hash << 5` trunca a 32 bits con signo, pero la resta y la suma del
            // original son de coma flotante y no desbordan: de ahí el i64.
            let mut hash: i64 = 0;
            for unit in branch.encode_utf16() {
                let shifted = i64::from((hash as i32).wrapping_shl(5));
                hash = i64::from(unit) + (shifted - hash);
            }
            let hue = hash.abs() % 360;
            format!("oklch(0.68 0.13 {hue})")
        }
    }
}

// Paleta cerrada del grafo (tema «Señal»): 8 tonos cada 45° con la misma
// luminosidad OKLCH. El valor real depende del tema (claro u oscuro), así que
// viajan como tokens (`var(--lane-N)`) que la interfaz resuelve con su paleta.
// El carril 0 (naranja señal) es siempre el de HEAD.
pub const GRAPH_COLORS: [&str; 8] = [
    "var(--lane-0)",
    "var(--lane-1)",
    "var(--lane-2)",
    "var(--lane-3)",
    "var(--lane-4)",
    "var(--lane-5)",
    "var(--lane-6)",
    "var(--lane-7)",
];

/// El color depende del carril, no del nombre de la rama.
pub fn get_graph_color(lane_index: usize) -> &'static str {
    GRAPH_COLORS[lane_index % GRAPH_COLORS.len()]
}

/// Un color de los que produce este módulo, ya interpretado.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpec {
    /// `oklch(L C H)`: luminosidad 0..1, croma y tono en grados.
    Oklch { l: f32, c: f32, h: f32 },
    /// `var(--nombre)`: un color del tema (`primary`, `warning`).
    ThemeToken(String),
}

impl ColorSpec {
    /// sRGB en 0..1 de un color `oklch`; `None` para los que dependen del tema.
    pub fn to_srgb(&self) -> Option<[f32; 3]> {
        match self {
            ColorSpec::Oklch { l, c, h } => Some(oklch_to_srgb(*l, *c, *h)),
            ColorSpec::ThemeToken(_) => None,
        }
    }
}

/// Interpreta `oklch(L C H)` o `var(--token)`. `None` para cualquier otro texto.
pub fn parse_css_color(color: &str) -> Option<ColorSpec> {
    let color = color.trim();

    if let Some(arguments) = color
        .strip_prefix("oklch(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let mut numbers = arguments.split_whitespace().map(str::parse::<f32>);
        let (l, c, h) = (
            numbers.next()?.ok()?,
            numbers.next()?.ok()?,
            numbers.next()?.ok()?,
        );
        return numbers
            .next()
            .is_none()
            .then_some(ColorSpec::Oklch { l, c, h });
    }

    let token = color.strip_prefix("var(--")?.strip_suffix(')')?;
    (!token.is_empty()).then(|| ColorSpec::ThemeToken(token.to_string()))
}

/// OKLCH → sRGB (con gamma, recortado a 0..1).
pub fn oklch_to_srgb(lightness: f32, chroma: f32, hue_degrees: f32) -> [f32; 3] {
    let hue = hue_degrees.to_radians();
    let (a, b) = (chroma * hue.cos(), chroma * hue.sin());

    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);

    let linear = [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ];

    linear.map(|channel| {
        let encoded = if channel <= 0.003_130_8 {
            12.92 * channel
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        };
        encoded.clamp(0.0, 1.0)
    })
}

// ---------------------------------------------------------------------------
// Refs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefType {
    /// `HEAD` suelto: HEAD desanclado en este commit.
    Head,
    #[default]
    Local,
    Tag,
    Remote,
}

impl RefType {
    pub fn as_str(self) -> &'static str {
        match self {
            RefType::Head => "head",
            RefType::Local => "local",
            RefType::Tag => "tag",
            RefType::Remote => "remote",
        }
    }

    fn order(self) -> u8 {
        match self {
            RefType::Head => 0,
            RefType::Local => 1,
            RefType::Tag => 2,
            RefType::Remote => 3,
        }
    }
}

/// Una ref que apunta a un commit.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    /// Nombre corto: `main`, `origin/main`, `v1.0`, `HEAD`.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: RefType,
    /// Marca dónde está HEAD (la rama actual, o `HEAD` si está desanclado).
    #[serde(default)]
    pub is_head: bool,
    #[serde(default)]
    pub color: String,
}

/// El log se pide con `--decorate=full`, así que cada ref llega con su ruta
/// completa y el tipo se lee sin adivinar.
///
/// Antes se deducía del texto corto con "si tiene barra es remota", y cualquier
/// rama local con barra (`feature/login`) se tomaba por remota: salía con icono
/// de nube en el historial y el doble click ofrecía crear un tracking de una
/// rama que ya era tuya. La lectura del formato corto se conserva como respaldo
/// para refs que no vengan decoradas.
fn classify_ref(part: &str) -> (&str, RefType) {
    const FULL_REF_PREFIXES: [(&str, RefType); 3] = [
        ("refs/heads/", RefType::Local),
        ("refs/remotes/", RefType::Remote),
        ("refs/tags/", RefType::Tag),
    ];

    for (prefix, kind) in FULL_REF_PREFIXES {
        if let Some(name) = part.strip_prefix(prefix) {
            return (name, kind);
        }
    }

    if part.contains('/') || part.starts_with("origin") {
        return (
            part.strip_prefix("remotes/").unwrap_or(part),
            RefType::Remote,
        );
    }

    (part, RefType::Local)
}

/// Refs de un commit a partir de la decoración de `git log` (`%D`).
pub fn parse_refs(refs_string: &str) -> Vec<CommitRef> {
    let mut refs = Vec::new();

    for part in refs_string
        .split(',')
        .map(js_trim)
        .filter(|part| !part.is_empty())
    {
        // `origin/HEAD` es solo la rama por defecto del remoto, no dónde estamos.
        if part.ends_with("/HEAD") {
            continue;
        }

        // `HEAD` suelto: el repo está en HEAD desanclado, así que ninguna rama
        // marca la posición actual y hay que representarla con su propia ref.
        if part == "HEAD" {
            refs.push(CommitRef {
                name: "HEAD".to_string(),
                kind: RefType::Head,
                is_head: true,
                color: WARNING_COLOR.to_string(),
            });
            continue;
        }

        // HEAD siempre apunta a una rama local: se fuerza el tipo para que el
        // respaldo del formato corto no se confunda con un nombre con barra.
        if let Some(target) = part.strip_prefix("HEAD ->") {
            let (name, _) = classify_ref(js_trim(target));
            refs.push(CommitRef {
                name: name.to_string(),
                kind: RefType::Local,
                is_head: true,
                color: get_branch_color(name),
            });
            continue;
        }

        if let Some(tag) = part.strip_prefix("tag:") {
            let (name, _) = classify_ref(js_trim(tag));
            refs.push(CommitRef {
                name: name.to_string(),
                kind: RefType::Tag,
                is_head: false,
                color: WARNING_COLOR.to_string(),
            });
            continue;
        }

        let (name, kind) = classify_ref(part);
        refs.push(CommitRef {
            name: name.to_string(),
            kind,
            is_head: false,
            color: match kind {
                RefType::Local => get_branch_color(name),
                RefType::Remote => PRIMARY_COLOR.to_string(),
                RefType::Tag | RefType::Head => WARNING_COLOR.to_string(),
            },
        });
    }

    // El marcador de HEAD va primero: la fila recorta los badges sobrantes, y el
    // que dice dónde estás no puede ser uno de los recortados. Entre remotos,
    // `origin/*` va primero y luego en orden alfabético ascendente (git `%D`
    // suele entregarlos al revés, haciendo que un segundo remoto tape a origin).
    fn ref_sort_key(commit_ref: &CommitRef) -> (bool, u8, (u8, &str, &str)) {
        let remote_order = if commit_ref.kind == RefType::Remote {
            match commit_ref.name.split_once('/') {
                Some(("origin", branch)) => (0u8, "origin", branch),
                Some((remote, branch)) => (1u8, remote, branch),
                None => (2u8, commit_ref.name.as_str(), ""),
            }
        } else {
            (0u8, "", "")
        };
        (!commit_ref.is_head, commit_ref.kind.order(), remote_order)
    }
    refs.sort_by(|left, right| ref_sort_key(left).cmp(&ref_sort_key(right)));
    refs
}

/// Badge de una fila del historial: una ref y, si es una rama local, los
/// remotos que tienen su copia en el mismo commit.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefBadge {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: RefType,
    #[serde(default)]
    pub is_head: bool,
    #[serde(default)]
    pub color: String,
    /// Solo en ramas locales: un nombre de remoto por cada copia plegada.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remotes: Option<Vec<String>>,
}

impl RefBadge {
    fn new(commit_ref: &CommitRef, remotes: Option<Vec<String>>) -> Self {
        Self {
            name: commit_ref.name.clone(),
            kind: commit_ref.kind,
            is_head: commit_ref.is_head,
            color: commit_ref.color.clone(),
            remotes,
        }
    }
}

/// Badges que se pintan en una fila del historial. Cuando una rama local y su
/// copia remota (`origin/main` junto a `main`) apuntan al mismo commit, se
/// muestran como un solo badge con una nube por remoto: decir "main" y
/// "origin/main" por separado ocupa el doble para contar lo mismo. Los remotos
/// sin rama local en el commit siguen con su nombre completo. No toca
/// `commit.refs`, que es lo que usan los menús y los flujos de checkout.
pub fn group_ref_badges(refs: &[CommitRef]) -> Vec<RefBadge> {
    let mut merged: HashSet<usize> = HashSet::new();
    let mut local_remotes: HashMap<usize, Vec<String>> = HashMap::new();

    for (index, commit_ref) in refs.iter().enumerate() {
        if commit_ref.kind != RefType::Local {
            continue;
        }

        // `origin/main` es la copia remota de `main` solo si lo que queda antes
        // del nombre es un remoto sin barras; `upstream/feature/main` no lo es.
        let suffix = format!("/{}", commit_ref.name);
        let mut remotes = Vec::new();
        for (candidate_index, candidate) in refs.iter().enumerate() {
            if candidate.kind != RefType::Remote {
                continue;
            }
            let Some(remote_name) = candidate.name.strip_suffix(&suffix) else {
                continue;
            };
            if remote_name.is_empty() || remote_name.contains('/') {
                continue;
            }
            merged.insert(candidate_index);
            remotes.push(remote_name.to_string());
        }
        local_remotes.insert(index, remotes);
    }

    refs.iter()
        .enumerate()
        .filter(|(index, _)| !merged.contains(index))
        .map(|(index, commit_ref)| RefBadge::new(commit_ref, local_remotes.remove(&index)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitRefBadgesOptions {
    /// Separación entre badges, en px.
    pub gap: f64,
    /// Ancho del chip "+N".
    pub more_width: f64,
}

impl Default for FitRefBadgesOptions {
    fn default() -> Self {
        Self {
            gap: 4.0,
            more_width: 34.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FittedBadges<T> {
    pub visible: Vec<T>,
    /// Cuántos quedan fuera; la fila los resume en un chip "+N".
    pub hidden: usize,
}

/// Cuántos badges caben en el ancho disponible sin recortar ninguno. Devuelve
/// los que se muestran y cuántos quedan fuera, que la fila resume en un chip
/// "+N". El primero (HEAD, cuando lo hay) se muestra siempre: la fila no puede
/// dejar de decir dónde estás. `measure` da el ancho en px de un badge.
pub fn fit_ref_badges<T: Clone>(
    badges: &[T],
    available_width: f64,
    measure: impl Fn(&T) -> f64,
    options: FitRefBadgesOptions,
) -> FittedBadges<T> {
    let everything = || FittedBadges {
        visible: badges.to_vec(),
        hidden: 0,
    };
    if !available_width.is_finite() || available_width <= 0.0 || badges.len() <= 1 {
        return everything();
    }

    let FitRefBadgesOptions { gap, more_width } = options;
    let widths: Vec<f64> = badges.iter().map(measure).collect();
    let total_width = widths.iter().sum::<f64>() + gap * (badges.len() - 1) as f64;
    if total_width <= available_width {
        return everything();
    }

    // Hay que dejar sitio al chip "+N" además de a los badges que se queden.
    let mut used = 0.0;
    let mut count = 0;
    for width in widths {
        let next = used + if count > 0 { gap } else { 0.0 } + width;
        if next + gap + more_width > available_width {
            break;
        }
        used = next;
        count += 1;
    }

    let count = count.max(1);
    FittedBadges {
        visible: badges[..count].to_vec(),
        hidden: badges.len() - count,
    }
}

// ---------------------------------------------------------------------------
// Grafo
// ---------------------------------------------------------------------------

/// Lo que el grafo y la pertenencia de rama necesitan saber de un commit. La
/// interfaz puede implementarlo para su propio tipo de fila.
pub trait HistoryCommit {
    /// Hash completo (o el corto si no hay otro): `commit.fullHash || commit.hash`.
    fn graph_hash(&self) -> &str;
    /// Hashes de los padres separados por espacios (`%P`).
    fn parents(&self) -> &str;
    fn refs(&self) -> &[CommitRef] {
        &[]
    }
}

fn parent_hashes(parents: &str) -> Vec<&str> {
    let mut unique = Vec::new();
    for hash in js_split_whitespace(parents) {
        if !unique.contains(&hash) {
            unique.push(hash);
        }
    }
    unique
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GraphEdgeType {
    /// Un carril que sigue pero cambia de columna.
    #[serde(rename = "lane-shift")]
    LaneShift,
    /// Del commit a su primer padre.
    #[serde(rename = "parent")]
    Parent,
    /// Del commit a un padre adicional (merge).
    #[serde(rename = "merge")]
    Merge,
}

impl GraphEdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            GraphEdgeType::LaneShift => "lane-shift",
            GraphEdgeType::Parent => "parent",
            GraphEdgeType::Merge => "merge",
        }
    }
}

/// Color de un carril o de una línea del grafo, en texto CSS como los demás.
///
/// Casi siempre es uno de [`GRAPH_COLORS`] (prestado: ni una reserva de
/// memoria por carril y fila, y copiarlo es copiar un puntero); solo el
/// grafo de respaldo ([`CommitGraph::fallback`]) lleva el color de la rama,
/// que se calcula y es propio. Se serializa igual que un `String`.
pub type GraphColor = Cow<'static, str>;

/// Conexión entre la fila de un commit y la siguiente: del carril `from` (a
/// media altura de la fila) al carril `to` (en el borde inferior).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub color: GraphColor,
    #[serde(rename = "type")]
    pub kind: GraphEdgeType,
}

/// Un carril tal como llega a la fila de un commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLane {
    pub index: usize,
    pub color: GraphColor,
    pub active: bool,
    /// Tiene línea por encima (desde el borde superior hasta media altura).
    pub arrives: bool,
    /// Sigue recto hasta la fila siguiente, en la misma columna.
    pub continues: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitGraph {
    /// Carril del commit.
    pub lane: usize,
    // El punto del commit se pinta con el color de su carril, para que
    // coincida siempre con la línea de la que cuelga.
    pub color: GraphColor,
    /// Carriles que ocupa la fila (para el ancho del grafo).
    pub lane_count: usize,
    pub lanes: Vec<GraphLane>,
    pub edges: Vec<GraphEdge>,
}

impl CommitGraph {
    /// Lo que pinta `CommitHistoryList` para un commit sin grafo calculado: un
    /// único carril con el color del commit.
    pub fn fallback(color: &str) -> Self {
        let color: GraphColor = if color.is_empty() {
            Cow::Borrowed(PRIMARY_COLOR)
        } else {
            Cow::Owned(color.to_string())
        };
        Self {
            lane: 0,
            color: color.clone(),
            lane_count: 1,
            lanes: vec![GraphLane {
                index: 0,
                color,
                active: true,
                arrives: true,
                continues: false,
            }],
            edges: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct Lane {
    hash: String,
    color: GraphColor,
}

/// Calculates the lanes and connections needed to render a Git history whose
/// commits always come before their parents (`git log --date-order`, which the
/// app uses, or `--topo-order`).
///
/// Devuelve un [`CommitGraph`] por commit, en el mismo orden.
///
/// Reglas, en este orden de importancia:
///
/// - **La línea de HEAD va siempre en la columna 0.** Es la rama en la que
///   estás (con lo que el upstream tenga por delante): ninguna otra rama ocupa
///   esa columna, así que la historia principal es una línea recta a la
///   izquierda y lo demás cuelga de ella.
/// - **Los carriles no se mueven.** Cada carril se queda en su columna hasta
///   terminar y uno nuevo ocupa el primer hueco libre (a la derecha del commit
///   si es el padre de un merge). El original compactaba los huecos e
///   insertaba el padre del merge junto al commit, así que en cada merge todos
///   los carriles de la derecha se corrían una columna.
/// - **Cuando dos líneas van al mismo padre sigue la de la izquierda** y la
///   otra se le une con una curva. Antes terminaba siempre la del commit, y la
///   rama principal saltaba a la columna de cualquier rama que hubiera salido
///   de ella.
pub fn compute_commit_graph<C: HistoryCommit>(commits: &[C]) -> Vec<CommitGraph> {
    // El siguiente commit de la línea de HEAD que falta por mostrar: solo él
    // puede ocupar la columna 0, que con línea de HEAD no es de nadie más.
    // Un commit más antiguo de la línea que llegue antes por otro camino va a
    // otra columna y se une a la 0 cuando la línea lo alcanza.
    let mut main_next = head_line_tip(commits);
    let reserved = main_next.is_some().then_some(0);
    let mut lanes: Vec<Option<Lane>> = Vec::new();

    commits
        .iter()
        .map(|commit| {
            let commit_hash = commit.graph_hash();
            let lane_index = lanes
                .iter()
                .position(|lane| lane.as_ref().is_some_and(|lane| lane.hash == commit_hash));
            // Si el carril no existía, ningún hijo apunta a este commit: es una punta de
            // rama (o el commit más reciente) y no debe dibujarse línea por encima.
            let lane_arrives_from_above = lane_index.is_some();
            let current_lane = lane_index.unwrap_or_else(|| {
                let slot = slot_for(&mut lanes, commit_hash, None, main_next, reserved);
                lanes[slot] = Some(Lane {
                    hash: commit_hash.to_string(),
                    color: Cow::Borrowed(get_graph_color(slot)),
                });
                slot
            });
            let current_color = lanes[current_lane]
                .as_ref()
                .map(|lane| lane.color.clone())
                .unwrap_or_default();
            let previous_lanes = lanes.clone();
            let mut next_lanes = lanes.clone();
            let parents = parent_hashes(commit.parents());
            if main_next == Some(commit_hash) {
                main_next = parents.first().copied();
            }
            let mut joins = Vec::new();

            next_lanes[current_lane] = None;
            for (index, parent_hash) in parents.iter().enumerate() {
                let existing = next_lanes
                    .iter()
                    .position(|lane| lane.as_ref().is_some_and(|lane| lane.hash == *parent_hash));
                match existing {
                    // El primer padre ya tiene línea a la derecha: sigue la del
                    // commit y aquella se le une.
                    Some(other) if index == 0 && other > current_lane => {
                        next_lanes[other] = None;
                        joins.push(other);
                        next_lanes[current_lane] = Some(Lane {
                            hash: (*parent_hash).to_string(),
                            color: current_color.clone(),
                        });
                    }
                    Some(_) => {}
                    None => {
                        // El primer padre sigue en la columna del commit salvo
                        // que sea de la línea de HEAD y esta vaya por otra.
                        let slot = if index == 0
                            && next_lanes[current_lane].is_none()
                            && !(main_next == Some(*parent_hash) && reserved != Some(current_lane))
                        {
                            current_lane
                        } else {
                            slot_for(
                                &mut next_lanes,
                                parent_hash,
                                Some(current_lane),
                                main_next,
                                reserved,
                            )
                        };
                        let color = if slot == current_lane {
                            current_color.clone()
                        } else {
                            Cow::Borrowed(get_graph_color(slot))
                        };
                        next_lanes[slot] = Some(Lane {
                            hash: (*parent_hash).to_string(),
                            color,
                        });
                    }
                }
            }
            while next_lanes.last().is_some_and(Option::is_none) {
                next_lanes.pop();
            }

            let lane_at = |hash: &str| {
                next_lanes.iter().enumerate().find_map(|(index, lane)| {
                    lane.as_ref()
                        .filter(|lane| lane.hash == hash)
                        .map(|lane| (index, lane.color.clone()))
                })
            };
            let mut edges = Vec::new();
            for from in joins {
                if let Some(lane) = &previous_lanes[from] {
                    edges.push(GraphEdge {
                        from,
                        to: current_lane,
                        color: lane.color.clone(),
                        kind: GraphEdgeType::LaneShift,
                    });
                }
            }
            for (index, parent_hash) in parents.iter().enumerate() {
                if let Some((to, lane_color)) = lane_at(parent_hash) {
                    let first = index == 0;
                    edges.push(GraphEdge {
                        from: current_lane,
                        to,
                        // La línea a un padre de merge es de la rama que entra:
                        // lleva el color del carril en el que sigue.
                        color: if first {
                            current_color.clone()
                        } else {
                            lane_color
                        },
                        kind: if first {
                            GraphEdgeType::Parent
                        } else {
                            GraphEdgeType::Merge
                        },
                    });
                }
            }

            // Los huecos no se pintan: solo los carriles con algo dentro.
            let graph_lanes = previous_lanes
                .iter()
                .enumerate()
                .filter_map(|(index, lane)| {
                    let lane = lane.as_ref()?;
                    Some(GraphLane {
                        index,
                        color: lane.color.clone(),
                        active: true,
                        // Los demás carriles vienen de filas anteriores; el del propio
                        // commit solo si algún hijo lo había abierto.
                        arrives: index != current_lane || lane_arrives_from_above,
                        continues: next_lanes
                            .get(index)
                            .and_then(Option::as_ref)
                            .is_some_and(|next| next.hash == lane.hash),
                    })
                })
                .collect();

            let lane_count = 1.max(previous_lanes.len()).max(next_lanes.len());
            lanes = next_lanes;

            CommitGraph {
                lane: current_lane,
                color: current_color,
                lane_count,
                lanes: graph_lanes,
                edges,
            }
        })
        .collect()
}

/// Dónde empieza la línea de HEAD (HEAD y sus primeros padres): el primer
/// commit mostrado que llega a HEAD por primeros padres, que es HEAD o el
/// upstream si va por delante. `None` si HEAD no está entre los cargados.
fn head_line_tip<C: HistoryCommit>(commits: &[C]) -> Option<&str> {
    let head_hash = commits
        .iter()
        .find(|commit| is_head_commit(*commit))?
        .graph_hash();
    // Los hijos van antes que los padres: recorriendo al revés, cada commit
    // sabe ya si su primer padre llega a HEAD.
    let mut reaches: HashSet<&str> = HashSet::new();
    for commit in commits.iter().rev() {
        let hash = commit.graph_hash();
        let first_parent = parent_hashes(commit.parents()).first().copied();
        if hash == head_hash || first_parent.is_some_and(|parent| reaches.contains(parent)) {
            reaches.insert(hash);
        }
    }
    commits
        .iter()
        .map(HistoryCommit::graph_hash)
        .find(|hash| reaches.contains(hash))
}

/// Columna para un carril nuevo que lleva `hash`. El siguiente commit de la
/// línea de HEAD (`main_next`) va a la columna reservada; lo demás, al primer hueco a la derecha de `after` (el
/// padre de un merge sale hacia la derecha de su commit), si no al primero
/// libre, y si no hay ninguno a una columna nueva al final. La columna
/// reservada no se le da a nadie más.
fn slot_for(
    lanes: &mut Vec<Option<Lane>>,
    hash: &str,
    after: Option<usize>,
    main_next: Option<&str>,
    reserved: Option<usize>,
) -> usize {
    if let Some(reserved) = reserved.filter(|_| main_next == Some(hash)) {
        if lanes.len() <= reserved {
            lanes.resize(reserved + 1, None);
        }
        if lanes[reserved].is_none() {
            return reserved;
        }
    }
    let from = after.map_or(0, |after| after + 1);
    let slot = (from..lanes.len())
        .chain(0..from.min(lanes.len()))
        .find(|&index| lanes[index].is_none() && Some(index) != reserved);
    slot.unwrap_or_else(|| {
        let mut end = lanes.len();
        if Some(end) == reserved {
            lanes.push(None);
            end += 1;
        }
        lanes.push(None);
        end
    })
}

// ---------------------------------------------------------------------------
// Pertenencia a la rama
// ---------------------------------------------------------------------------

/// Qué relación tiene un commit con tu copia de trabajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchMembership {
    /// Se alcanza desde HEAD.
    pub in_current_branch: bool,
    /// Está en el upstream de la rama actual y todavía no lo has traído.
    pub in_upstream: bool,
}

/// Qué commits de los cargados están ya en tu copia de trabajo y cuáles no.
///
/// La app pide `git log --all`, así que el historial mezcla tus commits con los
/// de otras ramas y con los que trajo un fetch y todavía no has pulleado. Sin
/// distinguirlos, todo parece igual de tuyo.
///
/// No hace falta preguntarle nada a git: ya tenemos los padres de cada commit
/// para dibujar el grafo, así que alcanzar desde HEAD es recorrer ese mismo
/// grafo. `--date-order` garantiza que un commit va antes que sus padres, de
/// modo que la ventana cargada es cerrada hacia arriba y el recorrido es exacto
/// dentro de ella.
fn collect_reachable<'a, C: HistoryCommit>(
    start_hash: &'a str,
    by_hash: &HashMap<&'a str, &'a C>,
) -> HashSet<&'a str> {
    let mut reachable = HashSet::new();
    let mut pending = vec![start_hash];

    while let Some(hash) = pending.pop() {
        if hash.is_empty() || reachable.contains(hash) {
            continue;
        }

        // Fuera de la ventana cargada: sus padres tampoco están.
        let Some(&commit) = by_hash.get(hash) else {
            continue;
        };

        reachable.insert(hash);
        pending.extend(parent_hashes(commit.parents()));
    }

    reachable
}

/// Pertenencia de cada commit, en el mismo orden que la entrada.
pub fn compute_branch_membership<C: HistoryCommit>(commits: &[C]) -> Vec<BranchMembership> {
    let by_hash: HashMap<&str, &C> = commits
        .iter()
        .map(|commit| (commit.graph_hash(), commit))
        .collect();

    let head_commit = commits
        .iter()
        .find(|commit| commit.refs().iter().any(|commit_ref| commit_ref.is_head));
    let head_branch = head_commit.and_then(|commit| {
        commit
            .refs()
            .iter()
            .find(|commit_ref| commit_ref.is_head && commit_ref.kind == RefType::Local)
            .map(|commit_ref| commit_ref.name.as_str())
            .filter(|name| !name.is_empty())
    });

    // La rama de seguimiento de la actual: por ahí llegan los commits que solo te
    // faltan por pullear, que es lo accionable.
    let upstream_commit = head_branch.and_then(|head_branch| {
        commits.iter().find(|commit| {
            commit.refs().iter().any(|commit_ref| {
                commit_ref.kind == RefType::Remote
                    && commit_ref
                        .name
                        .split_once('/')
                        .map_or("", |(_, branch)| branch)
                        == head_branch
            })
        })
    });

    let head_hash = head_commit
        .map(HistoryCommit::graph_hash)
        .filter(|hash| !hash.is_empty());
    let in_branch = head_hash
        .map(|hash| collect_reachable(hash, &by_hash))
        .unwrap_or_default();

    let upstream_hash = upstream_commit
        .map(HistoryCommit::graph_hash)
        .filter(|hash| !hash.is_empty());
    let in_upstream = upstream_hash
        .map(|hash| collect_reachable(hash, &by_hash))
        .unwrap_or_default();

    commits
        .iter()
        .map(|commit| {
            // Sin HEAD localizado no se puede afirmar nada, y teñir el historial entero
            // por no saberlo sería peor que no decir nada.
            if head_hash.is_none() {
                return BranchMembership {
                    in_current_branch: true,
                    in_upstream: false,
                };
            }

            let hash = commit.graph_hash();
            BranchMembership {
                in_current_branch: in_branch.contains(hash),
                in_upstream: !in_branch.contains(hash) && in_upstream.contains(hash),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Filas del historial
// ---------------------------------------------------------------------------

/// Un commit tal como lo devuelve `git:getCommits`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawCommit {
    /// Hash abreviado.
    pub hash: String,
    pub full_hash: String,
    pub message: String,
    pub author: String,
    pub email: String,
    /// Fecha de autor en ISO 8601.
    pub date: String,
    pub body: String,
    /// Decoración (`HEAD -> refs/heads/main, refs/tags/v1`).
    pub refs: String,
    pub parents: String,
}

/// Fila del historial: el commit normalizado más su grafo y su pertenencia.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitRow {
    pub hash: String,
    pub full_hash: String,
    pub refs: Vec<CommitRef>,
    pub message: String,
    pub message_body: String,
    pub author: String,
    pub email: String,
    /// Fecha relativa ya formateada ("3 hours ago").
    pub date: String,
    /// Fecha original, para el detalle.
    pub full_date: String,
    /// Color de la rama del commit (respaldo cuando no hay grafo).
    pub color: String,
    pub parents: String,
    /// Lo pone [`build_commit_graph`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<CommitGraph>,
    /// Lo pone [`mark_branch_membership`]; `None` es "sin calcular".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_current_branch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_upstream: Option<bool>,
}

impl HistoryCommit for CommitRow {
    fn graph_hash(&self) -> &str {
        if self.full_hash.is_empty() {
            &self.hash
        } else {
            &self.full_hash
        }
    }

    fn parents(&self) -> &str {
        &self.parents
    }

    fn refs(&self) -> &[CommitRef] {
        &self.refs
    }
}

/// `normalizeCommitViewBranch`: sin rama conocida se habla de `HEAD`.
pub fn normalize_commit_view_branch(branch_name: Option<&str>) -> &str {
    branch_name
        .filter(|name| !name.is_empty())
        .unwrap_or("HEAD")
}

/// Fila del historial a partir de un commit crudo (`normalizeRawCommit` de
/// `CommitView.vue`). `current_branch` es la rama actual de la vista.
pub fn normalize_raw_commit(commit: &RawCommit, current_branch: &str) -> CommitRow {
    normalize_raw_commit_at(commit, current_branch, &Local::now())
}

/// [`normalize_raw_commit`] con el "ahora" (y su zona horaria) explícitos.
pub fn normalize_raw_commit_at<Tz: TimeZone>(
    commit: &RawCommit,
    current_branch: &str,
    now: &DateTime<Tz>,
) -> CommitRow {
    let parsed_refs = parse_refs(&commit.refs);
    let branch_name = parsed_refs
        .iter()
        .find(|commit_ref| commit_ref.kind == RefType::Local)
        .map_or(current_branch, |commit_ref| commit_ref.name.as_str());
    let color = get_branch_color(branch_name);

    CommitRow {
        hash: commit.hash.clone(),
        full_hash: commit.full_hash.clone(),
        refs: parsed_refs,
        message: commit.message.clone(),
        message_body: commit.body.clone(),
        author: commit.author.clone(),
        email: commit.email.clone(),
        date: format_relative_date_at(commit.date.as_str(), now),
        full_date: commit.date.clone(),
        color,
        parents: commit.parents.clone(),
        graph: None,
        in_current_branch: None,
        in_upstream: None,
    }
}

/// Pone en cada fila su grafo (`buildCommitGraph`).
pub fn build_commit_graph(mut commits: Vec<CommitRow>) -> Vec<CommitRow> {
    let graphs = compute_commit_graph(&commits);
    for (commit, graph) in commits.iter_mut().zip(graphs) {
        commit.graph = Some(graph);
    }
    commits
}

/// Pone en cada fila si está en tu rama o pendiente de pull (`markBranchMembership`).
pub fn mark_branch_membership(mut commits: Vec<CommitRow>) -> Vec<CommitRow> {
    let memberships = compute_branch_membership(&commits);
    for (commit, membership) in commits.iter_mut().zip(memberships) {
        commit.in_current_branch = Some(membership.in_current_branch);
        commit.in_upstream = Some(membership.in_upstream);
    }
    commits
}

/// Carriles que necesita el grafo de toda la lista (`graphLaneCount` de `CommitView.vue`).
pub fn graph_lane_count(commits: &[CommitRow]) -> usize {
    commits
        .iter()
        .map(|commit| {
            commit
                .graph
                .as_ref()
                .map_or(1, |graph| graph.lane_count.max(1))
        })
        .fold(1, usize::max)
}

/// El commit en el que está el working tree. HEAD viene marcado en las refs del
/// propio commit (`%D` de git log), así que la lista no necesita saber en qué
/// rama está el repo para señalar la posición.
pub fn is_head_commit(commit: &impl HistoryCommit) -> bool {
    commit.refs().iter().any(|commit_ref| commit_ref.is_head)
}

/// Por qué "Interactive rebase from here…" no puede abrirse desde un commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveRebaseRangeError {
    /// Hay un merge entre el commit y HEAD: el rebase lo aplanaría.
    MergeCommit,
    /// El commit no está en la historia de la rama actual (o no está cargado).
    NotInCurrentBranch,
}

impl InteractiveRebaseRangeError {
    /// El aviso que ve el usuario (los textos de `openInteractiveRebase`).
    pub fn message(self) -> &'static str {
        match self {
            InteractiveRebaseRangeError::MergeCommit => {
                "The range contains a merge commit; interactive rebase supports linear history only."
            }
            InteractiveRebaseRangeError::NotInCurrentBranch => {
                "Interactive rebase only works on commits of the current branch."
            }
        }
    }
}

/// Commits que reescribe "Interactive rebase from here…" desde `target_hash`
/// (hash completo): ese commit y los que hay encima hasta HEAD, del más
/// reciente al más antiguo, como en la lista.
///
/// El rango se arma siguiendo los padres desde HEAD, no tomando todas las filas
/// por encima del commit. La lista sale de `git log --all`: encima de HEAD
/// pueden ir commits de otras ramas y el de un stash (`refs/stash` es un merge
/// del working tree y el índice), y la app Electron los metía en el rango, de
/// modo que con un stash guardado se negaba siempre ("contains a merge
/// commit"). Lo que no se alcanza desde HEAD no se reescribe y no cuenta.
pub fn interactive_rebase_range<'a, C: HistoryCommit>(
    commits: &'a [C],
    target_hash: &str,
) -> Result<Vec<&'a C>, InteractiveRebaseRangeError> {
    let by_hash: HashMap<&str, &C> = commits
        .iter()
        .map(|commit| (commit.graph_hash(), commit))
        .collect();
    let mut current = commits
        .iter()
        .find(|commit| is_head_commit(*commit))
        .ok_or(InteractiveRebaseRangeError::NotInCurrentBranch)?;

    let mut range = Vec::new();
    loop {
        range.push(current);
        let parents = parent_hashes(current.parents());
        if parents.len() > 1 {
            return Err(InteractiveRebaseRangeError::MergeCommit);
        }
        if !target_hash.is_empty() && current.graph_hash() == target_hash {
            return Ok(range);
        }
        // Raíz alcanzada, o un padre fuera de la ventana cargada: con
        // `--date-order` todo lo que hay entre HEAD y un commit cargado de su
        // historia también está cargado, así que el commit no es de la rama.
        current = parents
            .first()
            .and_then(|parent| by_hash.get(parent).copied())
            .ok_or(InteractiveRebaseRangeError::NotInCurrentBranch)?;
    }
}

/// Decir por qué una fila está apagada, para que el atenuado no parezca un
/// fallo de pintura.
pub fn membership_title(
    in_current_branch: Option<bool>,
    in_upstream: Option<bool>,
) -> Option<&'static str> {
    if in_upstream == Some(true) {
        return Some("Already on your upstream — pull to get it");
    }

    if in_current_branch == Some(false) {
        return Some("Not in your current branch");
    }

    None
}

/// Tooltip de un badge: dónde está HEAD y qué copias remotas resume (la
/// etiqueta de `main` con su nube dice "In sync with origin/main"). En inglés,
/// como el resto de la interfaz.
pub fn badge_title(badge: &RefBadge) -> Option<String> {
    let mut parts = Vec::new();
    if badge.is_head {
        parts.push(if badge.kind == RefType::Head {
            "You are here · HEAD detached at this commit".to_string()
        } else {
            format!("You are here · HEAD is on {}", badge.name)
        });
    }
    let remotes: Vec<String> = badge
        .remotes
        .iter()
        .flatten()
        .map(|remote| format!("{remote}/{}", badge.name))
        .collect();
    if !remotes.is_empty() {
        parts.push(format!("In sync with {}", remotes.join(", ")));
    }

    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Tooltip del chip "+N": los badges que no caben, con sus copias remotas.
pub fn hidden_badge_title(badges: &[RefBadge], visible_count: usize) -> String {
    badges
        .iter()
        .skip(visible_count)
        .flat_map(|badge| {
            std::iter::once(badge.name.clone()).chain(
                badge
                    .remotes
                    .iter()
                    .flatten()
                    .map(|remote| format!("{remote}/{}", badge.name)),
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(full_hash: &str, hash: &str, parents: &str) -> CommitRow {
        CommitRow {
            full_hash: full_hash.to_string(),
            hash: hash.to_string(),
            parents: parents.to_string(),
            ..Default::default()
        }
    }

    fn colored(full_hash: &str, hash: &str, parents: &str, color: &str) -> CommitRow {
        CommitRow {
            color: color.to_string(),
            ..row(full_hash, hash, parents)
        }
    }

    fn graph(commit: &CommitRow) -> &CommitGraph {
        commit.graph.as_ref().expect("graph")
    }

    fn edge(from: usize, to: usize, color: &str, kind: GraphEdgeType) -> GraphEdge {
        GraphEdge {
            from,
            to,
            color: Cow::Owned(color.to_string()),
            kind,
        }
    }

    // --- buildCommitGraph ---

    #[test]
    fn keeps_the_first_parent_in_the_current_lane_and_opens_a_lane_for_merges() {
        let commits = build_commit_graph(vec![
            colored("merge", "merge", "main-parent feature-parent", "main-color"),
            colored("main-parent", "main-pa", "root", "main-color"),
            colored("feature-parent", "feature-pa", "root", "feature-color"),
            colored("root", "root", "", "main-color"),
        ]);

        // El color sale de la paleta del carril, no del `color` del commit (que
        // venía de un hash del nombre de rama y producía tonos que chocaban).
        let lane_zero_color = graph(&commits[0]).color.clone();

        assert_eq!(graph(&commits[0]).lane, 0);
        assert_eq!(graph(&commits[0]).lane_count, 2);
        assert_ne!(lane_zero_color, "main-color");
        // La línea al padre del merge lleva el color de la rama que entra (el
        // de su carril), no el del commit que hace el merge.
        assert_eq!(
            graph(&commits[0]).edges,
            [
                edge(0, 0, &lane_zero_color, GraphEdgeType::Parent),
                edge(0, 1, get_graph_color(1), GraphEdgeType::Merge),
            ]
        );
        assert_eq!(graph(&commits[1]).lane, 0);
        assert_eq!(graph(&commits[2]).lane, 1);
        // Carriles distintos, colores distintos.
        assert_ne!(graph(&commits[2]).color, lane_zero_color);
        // La fila conserva el resto del commit.
        assert_eq!(commits[2].color, "feature-color");
    }

    #[test]
    fn deduplicates_parents_already_represented_by_another_lane() {
        let commits = build_commit_graph(vec![
            colored("merge", "merge", "shared side", "main-color"),
            colored("shared", "shared", "root", "main-color"),
            colored("side", "side", "shared", "side-color"),
            colored("root", "root", "", "main-color"),
        ]);

        let parent_edges: Vec<String> = graph(&commits[0])
            .edges
            .iter()
            .map(|edge| format!("{}:{}", edge.from, edge.to))
            .collect();
        assert_eq!(parent_edges, ["0:0", "0:1"]);
    }

    #[test]
    fn does_not_draw_a_lane_above_a_commit_that_no_child_points_to() {
        let commits = build_commit_graph(vec![row("tip", "tip", "root"), row("root", "root", "")]);

        // El commit más reciente (y cualquier punta de rama) no recibe línea por
        // arriba: no hay nada encima de él. Hacia abajo conecta el edge al padre,
        // no la línea del carril (`continues` es para carriles de paso).
        assert!(!graph(&commits[0]).lanes[0].arrives);
        assert_eq!(graph(&commits[0]).edges.len(), 1);
        let only_edge = &graph(&commits[0]).edges[0];
        assert_eq!(
            (only_edge.from, only_edge.to, only_edge.kind),
            (0, 0, GraphEdgeType::Parent)
        );
        assert!(!only_edge.color.is_empty());
        // El siguiente sí la recibe, porque su hijo abrió el carril.
        assert!(graph(&commits[1]).lanes[0].arrives);
    }

    #[test]
    fn marks_a_branch_tip_opened_mid_history_as_not_arriving_either() {
        let commits = build_commit_graph(vec![
            row("merge", "merge", "main-parent feature-tip"),
            row("main-parent", "main-pa", "root"),
            row("feature-tip", "feature", "root"),
            row("root", "root", ""),
        ]);

        // El merge abre el carril de la rama, así que su punta sí llega desde arriba.
        let feature_row = graph(&commits[2]);
        assert!(feature_row.lanes[feature_row.lane].arrives);
    }

    #[test]
    fn follows_a_branch_through_to_the_commit_where_it_joins_again() {
        let graphs = compute_commit_graph(&[
            row("merge", "merge", "main-parent feature-tip"),
            row("main-parent", "main-pa", "root"),
            row("feature-tip", "feature", "root"),
            row("root", "root", ""),
        ]);

        // Fila del merge: un solo carril al llegar, dos al salir.
        assert_eq!(graphs[0].lanes.len(), 1);
        assert_eq!(graphs[0].lane_count, 2);
        // Fila de main-parent: el carril de la rama pasa de largo.
        assert_eq!(graphs[1].lanes.len(), 2);
        assert!(graphs[1].lanes[1].continues);
        assert!(graphs[1].lanes[1].arrives);
        assert!(graphs[1].lanes.iter().all(|lane| lane.active));
        // Fila de la punta: su padre ya tiene carril (el 0), así que el suyo se
        // cierra y la arista va en diagonal.
        assert_eq!(graphs[2].lane, 1);
        assert!(!graphs[2].lanes[1].continues);
        assert_eq!(
            graphs[2].edges,
            [edge(1, 0, get_graph_color(1), GraphEdgeType::Parent)]
        );
        // La raíz no tiene a dónde seguir.
        assert_eq!(graphs[3].lanes.len(), 1);
        assert!(graphs[3].edges.is_empty());
        assert_eq!(graphs[3].lane_count, 1);
    }

    #[test]
    fn keeps_each_lane_in_its_column_when_another_one_closes() {
        // Dos puntas sin relación: la primera termina y la siguiente punta
        // reutiliza su columna libre.
        let graphs = compute_commit_graph(&[
            row("a2", "a2", "a1"),
            row("b1", "b1", ""),
            row("a1", "a1", ""),
        ]);

        assert_eq!(graphs[1].lane, 1);
        assert!(!graphs[1].lanes[1].arrives);
        assert_eq!(graphs[2].lane, 0);

        let graphs = compute_commit_graph(&[
            row("a1", "a1", ""),
            row("b2", "b2", "b1"),
            row("c2", "c2", "c1"),
            row("b1", "b1", ""),
            row("c1", "c1", ""),
        ]);
        // Al cerrarse el carril 0 (b1 es raíz), el de c1 sigue en la columna 1:
        // nada se corre ni se curva, y el hueco no se pinta.
        assert_eq!(graphs[3].lane, 0);
        assert!(graphs[3].edges.is_empty());
        assert_eq!(graphs[4].lane, 1);
        assert_eq!(
            graphs[4]
                .lanes
                .iter()
                .map(|lane| lane.index)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    fn head_row(full_hash: &str, parents: &str) -> CommitRow {
        CommitRow {
            refs: parse_refs("HEAD -> refs/heads/main"),
            ..row(full_hash, full_hash, parents)
        }
    }

    /// Una rama nueva aparece antes que HEAD en la lista (es más reciente) y
    /// sale de un commit de la rama principal. Antes la punta nueva tomaba la
    /// columna 0 y la rama principal saltaba a la columna de la rama cuando
    /// compartían padre; ahora la principal va siempre por la 0 y es la rama
    /// la que se une a ella.
    #[test]
    fn keeps_the_head_line_in_the_first_column() {
        let graphs = compute_commit_graph(&[
            row("feature", "feature", "base"),
            head_row("main2", "main1"),
            row("main1", "main1", "base"),
            row("base", "base", ""),
        ]);

        assert_eq!(graphs[0].lane, 1, "la rama nueva no ocupa la columna 0");
        let head_lanes: Vec<usize> = graphs[1..].iter().map(|graph| graph.lane).collect();
        assert_eq!(head_lanes, [0, 0, 0]);
        // main1 sigue en su columna y la línea de feature (que también va a
        // base) se le une con una curva desde la columna 1.
        assert_eq!(
            graphs[2]
                .edges
                .iter()
                .map(|edge| (edge.from, edge.to, edge.kind))
                .collect::<Vec<_>>(),
            [
                (1, 0, GraphEdgeType::LaneShift),
                (0, 0, GraphEdgeType::Parent)
            ]
        );
    }

    /// Con el upstream por delante (pull pendiente) la línea de HEAD empieza
    /// en la punta del upstream: también va por la columna 0.
    #[test]
    fn puts_the_upstream_ahead_of_head_in_the_first_column_too() {
        let graphs = compute_commit_graph(&[
            row("origin-main", "origin-main", "main"),
            row("other", "other", "main"),
            head_row("main", "base"),
            row("base", "base", ""),
        ]);

        assert_eq!(graphs[0].lane, 0);
        assert_eq!(graphs[1].lane, 1);
        assert_eq!(graphs[2].lane, 0);
        assert_eq!(graphs[3].lane, 0);
    }

    /// Una rama que hace merge de `development` una y otra vez mientras otras
    /// ramas salen de `development`: los commits de `development` quedan
    /// esperando a sus otros hijos y sus carriles se acumulan. Antes cada merge
    /// corría todos esos carriles una columna (una escalera de curvas); ahora
    /// cada carril sigue recto en su columna.
    #[test]
    fn repeated_merges_do_not_shift_the_waiting_lanes() {
        let mut rows = Vec::new();
        for i in (1..=4).rev() {
            rows.push(row(&format!("f{i}"), &format!("f{i}"), &format!("d{i}")));
        }
        for i in (1..=4).rev() {
            let previous = if i == 1 {
                "init".to_string()
            } else {
                format!("m{}", i - 1)
            };
            rows.push(row(
                &format!("m{i}"),
                &format!("m{i}"),
                &format!("{previous} d{i}"),
            ));
        }
        for i in (1..=4).rev() {
            let previous = if i == 1 {
                "init".to_string()
            } else {
                format!("d{}", i - 1)
            };
            rows.push(row(&format!("d{i}"), &format!("d{i}"), &previous));
        }
        rows.push(row("init", "init", ""));

        let graphs = compute_commit_graph(&rows);

        // En las filas de los merges ningún carril se mueve.
        assert!(graphs[4..8].iter().all(|graph| {
            graph
                .edges
                .iter()
                .all(|edge| edge.kind != GraphEdgeType::LaneShift)
        }));
        // La cadena de merges va siempre por la misma columna.
        let merge_lanes: Vec<usize> = graphs[4..8].iter().map(|graph| graph.lane).collect();
        assert_eq!(merge_lanes, [4, 4, 4, 4]);
        // Y los carriles de las ramas esperan rectos: un carril que llega a una
        // fila sigue en la siguiente mientras no sea el del commit.
        for graph in &graphs[4..8] {
            for lane in graph.lanes.iter().filter(|lane| lane.index < 4) {
                assert!(lane.continues, "{graph:?}");
            }
        }
    }

    #[test]
    fn falls_back_to_the_short_hash_and_ignores_repeated_parents() {
        let graphs =
            compute_commit_graph(&[row("", "tip", "root  root\troot"), row("", "root", "")]);

        assert_eq!(graphs[0].edges.len(), 1);
        assert_eq!(graphs[1].lane, 0);
        assert!(graphs[1].lanes[0].arrives);
        assert_eq!(compute_commit_graph::<CommitRow>(&[]), []);
    }

    #[test]
    fn cycles_through_the_closed_palette() {
        assert_eq!(get_graph_color(0), "var(--lane-0)");
        assert_eq!(get_graph_color(7), "var(--lane-7)");
        assert_eq!(get_graph_color(8), get_graph_color(0));
    }

    #[test]
    fn serializes_the_graph_with_the_same_field_names_as_the_renderer() {
        let commits = build_commit_graph(vec![row("tip", "tip", "root"), row("root", "root", "")]);

        assert_eq!(
            serde_json::to_value(graph(&commits[1])).unwrap(),
            serde_json::json!({
                "lane": 0,
                "color": "var(--lane-0)",
                "laneCount": 1,
                "lanes": [{
                    "index": 0,
                    "color": "var(--lane-0)",
                    "active": true,
                    "arrives": true,
                    "continues": false
                }],
                "edges": []
            })
        );
        assert_eq!(
            serde_json::to_value(&graph(&commits[0]).edges[0]).unwrap(),
            serde_json::json!({ "from": 0, "to": 0, "color": "var(--lane-0)", "type": "parent" })
        );
        assert_eq!(GraphEdgeType::LaneShift.as_str(), "lane-shift");
        assert_eq!(
            serde_json::to_value(GraphEdgeType::LaneShift).unwrap(),
            "lane-shift"
        );
    }

    // --- parseRefs ---

    fn summary(refs: &[CommitRef]) -> Vec<(&str, RefType, bool)> {
        refs.iter()
            .map(|commit_ref| {
                (
                    commit_ref.name.as_str(),
                    commit_ref.kind,
                    commit_ref.is_head,
                )
            })
            .collect()
    }

    fn names(refs: &[CommitRef]) -> Vec<&str> {
        refs.iter()
            .map(|commit_ref| commit_ref.name.as_str())
            .collect()
    }

    #[test]
    fn marks_the_branch_head_points_at_so_the_row_can_show_the_current_position() {
        let refs = parse_refs("HEAD -> main, origin/main, tag: v0.2.1");

        assert_eq!(
            summary(&refs),
            [
                ("main", RefType::Local, true),
                ("v0.2.1", RefType::Tag, false),
                ("origin/main", RefType::Remote, false),
            ]
        );
    }

    #[test]
    fn keeps_a_ref_for_a_detached_head_where_no_branch_marks_the_position() {
        let refs = parse_refs("HEAD, tag: v0.2.0");

        assert_eq!(summary(&refs)[0], ("HEAD", RefType::Head, true));
    }

    #[test]
    fn ignores_the_default_branch_pointer_of_a_remote() {
        let refs = parse_refs("origin/HEAD, origin/main");

        assert_eq!(names(&refs), ["origin/main"]);
        assert!(!refs.iter().any(|commit_ref| commit_ref.is_head));
    }

    // El log se pide con `--decorate=full`: el tipo se lee de la ruta, no del
    // texto. Con el formato corto, `feature/login` (rama LOCAL) se tomaba por
    // remota y el doble click ofrecía crear un tracking de una rama ya tuya.
    #[test]
    fn reads_the_ref_type_from_the_full_path_instead_of_guessing() {
        let refs = parse_refs(
            "HEAD -> refs/heads/feature/login, refs/remotes/origin/feature/login, tag: refs/tags/v1.0",
        );

        assert_eq!(
            summary(&refs),
            [
                ("feature/login", RefType::Local, true),
                ("v1.0", RefType::Tag, false),
                ("origin/feature/login", RefType::Remote, false),
            ]
        );
    }

    #[test]
    fn keeps_a_slashed_local_branch_local_when_it_is_not_the_current_one() {
        let refs = parse_refs("refs/heads/fix/crash");

        assert_eq!(summary(&refs), [("fix/crash", RefType::Local, false)]);
    }

    #[test]
    fn still_ignores_the_remote_default_pointer_when_refs_come_decorated() {
        let refs = parse_refs("refs/remotes/origin/HEAD, refs/remotes/origin/main");

        assert_eq!(names(&refs), ["origin/main"]);
    }

    #[test]
    fn puts_the_current_position_first_before_local_branches_that_only_share_the_commit() {
        let refs = parse_refs("feature, HEAD -> main");

        assert_eq!(names(&refs), ["main", "feature"]);
    }

    #[test]
    fn colours_each_ref_by_its_kind() {
        let refs = parse_refs(
            "HEAD -> refs/heads/main, refs/heads/topic, refs/remotes/origin/main, tag: refs/tags/v1",
        );

        let color_of = |name: &str| {
            refs.iter()
                .find(|commit_ref| commit_ref.name == name)
                .map(|commit_ref| commit_ref.color.as_str())
        };
        assert_eq!(color_of("main"), Some("oklch(0.72 0.12 180)"));
        assert_eq!(color_of("topic"), Some(get_branch_color("topic").as_str()));
        assert_eq!(color_of("origin/main"), Some("var(--primary)"));
        assert_eq!(color_of("v1"), Some("var(--warning)"));
        assert_eq!(parse_refs("HEAD")[0].color, "var(--warning)");
        assert_eq!(parse_refs(""), []);
        // Respaldo del formato corto.
        assert_eq!(
            summary(&parse_refs("remotes/origin/dev, origin")),
            [
                ("origin/dev", RefType::Remote, false),
                ("origin", RefType::Remote, false)
            ]
        );
    }

    #[test]
    fn prioritizes_origin_and_orders_remotes_ascending() {
        let refs = parse_refs(
            "refs/remotes/web/release/1.x, refs/remotes/web/main, refs/remotes/origin/release/1.x, refs/remotes/origin/main",
        );
        assert_eq!(
            names(&refs),
            [
                "origin/main",
                "origin/release/1.x",
                "web/main",
                "web/release/1.x"
            ]
        );
    }

    #[test]
    fn gives_every_branch_a_stable_colour() {
        assert_eq!(get_branch_color(""), "var(--primary)");
        assert_eq!(get_branch_color("master"), "oklch(0.72 0.12 180)");
        assert_eq!(get_branch_color("dev"), "oklch(0.68 0.14 330)");
        // Mismos tonos que el hash del original (comprobados en Node).
        assert_eq!(get_branch_color("feature"), "oklch(0.68 0.13 22)");
        assert_eq!(get_branch_color("feature/login"), "oklch(0.68 0.13 328)");
        assert_eq!(get_branch_color("topic"), "oklch(0.68 0.13 303)");
        // Nombres largos: el hash pasa de 32 bits y el original no lo trunca.
        assert_eq!(
            get_branch_color("release/2026.09-candidate-with-a-long-name"),
            "oklch(0.68 0.13 269)"
        );
        assert_eq!(get_branch_color(&"x".repeat(200)), "oklch(0.68 0.13 296)");
        // Fuera de ASCII se cuenta en unidades UTF-16, como `charCodeAt`.
        assert_eq!(get_branch_color("ñandú"), "oklch(0.68 0.13 348)");
        assert_eq!(get_branch_color("😀-emoji"), "oklch(0.68 0.13 68)");
    }

    #[test]
    fn interprets_the_css_colours_it_produces() {
        assert_eq!(
            parse_css_color("oklch(0.68 0.12 264)"),
            Some(ColorSpec::Oklch {
                l: 0.68,
                c: 0.12,
                h: 264.0
            })
        );
        assert_eq!(
            parse_css_color("var(--primary)"),
            Some(ColorSpec::ThemeToken("primary".to_string()))
        );
        assert_eq!(parse_css_color("#409eff"), None);
        assert_eq!(parse_css_color("oklch(0.68 0.12)"), None);
        assert_eq!(ColorSpec::ThemeToken("warning".to_string()).to_srgb(), None);

        let close = |actual: [f32; 3], expected: [f32; 3]| {
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 0.01)
        };
        assert!(close(oklch_to_srgb(1.0, 0.0, 0.0), [1.0, 1.0, 1.0]));
        assert!(close(oklch_to_srgb(0.0, 0.0, 0.0), [0.0, 0.0, 0.0]));
        // Rojo y azul puros de sRGB en OKLCH.
        assert!(close(
            oklch_to_srgb(0.627_955, 0.257_683, 29.233_885),
            [1.0, 0.0, 0.0]
        ));
        assert!(close(
            oklch_to_srgb(0.452_014, 0.313_214, 264.052),
            [0.0, 0.0, 1.0]
        ));
        // Los carriles dependen del tema: los resuelve la interfaz.
        for (index, color) in GRAPH_COLORS.iter().enumerate() {
            assert_eq!(
                parse_css_color(color),
                Some(ColorSpec::ThemeToken(format!("lane-{index}")))
            );
        }
    }

    // --- markBranchMembership ---

    fn commit(hash: &str, parents: &str, refs: Vec<CommitRef>) -> CommitRow {
        CommitRow {
            refs,
            ..row(hash, hash, parents)
        }
    }

    fn commit_ref(name: &str, kind: RefType, is_head: bool) -> CommitRef {
        CommitRef {
            name: name.to_string(),
            kind,
            is_head,
            color: String::new(),
        }
    }

    fn local_head(name: &str) -> CommitRef {
        commit_ref(name, RefType::Local, true)
    }

    fn remote(name: &str) -> CommitRef {
        commit_ref(name, RefType::Remote, false)
    }

    fn find<'a>(marked: &'a [CommitRow], hash: &str) -> &'a CommitRow {
        marked
            .iter()
            .find(|item| item.hash == hash)
            .expect("commit")
    }

    fn membership(marked: &[CommitRow], hash: &str) -> (Option<bool>, Option<bool>) {
        let item = find(marked, hash);
        (item.in_current_branch, item.in_upstream)
    }

    fn range_hashes(
        commits: &[CommitRow],
        target: &str,
    ) -> Result<Vec<String>, InteractiveRebaseRangeError> {
        interactive_rebase_range(commits, target)
            .map(|range| range.iter().map(|commit| commit.hash.clone()).collect())
    }

    /*
     * `git log --all` pone el stash (un merge del working tree y el índice, más
     * el commit del índice) encima de HEAD. La app Electron tomaba todas las
     * filas por encima del commit y se negaba siempre que hubiera un stash.
     */
    #[test]
    fn the_interactive_rebase_range_ignores_stashes_and_other_branches() {
        let commits = vec![
            commit(
                "stash",
                "tercero indice",
                vec![commit_ref("refs/stash", RefType::Local, false)],
            ),
            commit("indice", "tercero", vec![]),
            commit(
                "otra",
                "primero",
                vec![commit_ref("feature", RefType::Local, false)],
            ),
            commit("tercero", "segundo", vec![local_head("main")]),
            commit("segundo", "primero", vec![]),
            commit("primero", "", vec![]),
        ];

        assert_eq!(
            range_hashes(&commits, "segundo"),
            Ok(vec!["tercero".to_string(), "segundo".to_string()])
        );
        assert_eq!(
            range_hashes(&commits, "primero").map(|range| range.len()),
            Ok(3)
        );
        assert_eq!(
            range_hashes(&commits, "tercero"),
            Ok(vec!["tercero".to_string()])
        );
        // Ni el stash ni la otra rama son de la rama actual.
        for outside in ["stash", "indice", "otra", "nada"] {
            assert_eq!(
                range_hashes(&commits, outside),
                Err(InteractiveRebaseRangeError::NotInCurrentBranch),
                "{outside}"
            );
        }
    }

    #[test]
    fn the_interactive_rebase_range_refuses_merges_between_the_commit_and_head() {
        let commits = vec![
            commit("merge", "main1 lado", vec![local_head("main")]),
            commit("lado", "base", vec![]),
            commit("main1", "base", vec![]),
            commit("base", "", vec![]),
        ];

        assert_eq!(
            range_hashes(&commits, "main1"),
            Err(InteractiveRebaseRangeError::MergeCommit)
        );
        assert_eq!(
            InteractiveRebaseRangeError::MergeCommit.message(),
            "The range contains a merge commit; interactive rebase supports linear history only."
        );
        // Sin HEAD en la lista no se puede afirmar que el commit sea de la rama.
        assert_eq!(
            range_hashes(&commits[1..], "base"),
            Err(InteractiveRebaseRangeError::NotInCurrentBranch)
        );
        assert_eq!(
            InteractiveRebaseRangeError::NotInCurrentBranch.message(),
            "Interactive rebase only works on commits of the current branch."
        );
    }

    // El caso que motivó esto: `git log --all` mezcla tus commits con los de otra
    // rama, y hasta ahora se veían igual de tuyos.
    #[test]
    fn separates_what_is_in_your_branch_from_what_is_not() {
        let marked = mark_branch_membership(vec![
            commit(
                "otra2",
                "otra1",
                vec![commit_ref("feature", RefType::Local, false)],
            ),
            commit("otra1", "base", vec![]),
            commit("mio", "base", vec![local_head("main")]),
            commit("base", "", vec![]),
        ]);

        assert_eq!(find(&marked, "mio").in_current_branch, Some(true));
        assert_eq!(find(&marked, "base").in_current_branch, Some(true));
        assert_eq!(find(&marked, "otra1").in_current_branch, Some(false));
        assert_eq!(find(&marked, "otra2").in_current_branch, Some(false));
    }

    // Lo que te falta por pullear es accionable y no debe perderse entre lo demás.
    #[test]
    fn tells_apart_what_your_upstream_has_and_you_have_not_pulled() {
        let marked = mark_branch_membership(vec![
            commit("sinpullear", "mio", vec![remote("origin/main")]),
            commit("mio", "base", vec![local_head("main")]),
            commit("base", "", vec![]),
        ]);

        assert_eq!(membership(&marked, "sinpullear"), (Some(false), Some(true)));
        assert_eq!(membership(&marked, "mio"), (Some(true), Some(false)));
    }

    #[test]
    fn does_not_confuse_another_remote_branch_with_your_upstream() {
        let marked = mark_branch_membership(vec![
            commit("otro", "base", vec![remote("origin/feature")]),
            commit("mio", "base", vec![local_head("main")]),
            commit("base", "", vec![]),
        ]);

        assert_eq!(membership(&marked, "otro"), (Some(false), Some(false)));
    }

    #[test]
    fn follows_both_parents_of_a_merge() {
        let marked = mark_branch_membership(vec![
            commit("merge", "mio traido", vec![local_head("main")]),
            commit("traido", "base", vec![]),
            commit("mio", "base", vec![]),
            commit("base", "", vec![]),
        ]);

        assert!(
            marked
                .iter()
                .all(|item| item.in_current_branch == Some(true))
        );
    }

    // Un padre fuera de la ventana no invalida lo que sí está dentro.
    #[test]
    fn stops_at_the_edge_of_what_is_loaded() {
        let marked = mark_branch_membership(vec![commit(
            "mio",
            "fuera-de-la-ventana",
            vec![local_head("main")],
        )]);

        assert_eq!(marked[0].in_current_branch, Some(true));
    }

    // Sin saber dónde está HEAD, teñir el historial entero sería peor que callar.
    #[test]
    fn claims_nothing_when_head_is_not_in_the_window() {
        let marked =
            mark_branch_membership(vec![commit("a", "b", vec![]), commit("b", "", vec![])]);

        assert!(
            marked
                .iter()
                .all(|item| item.in_current_branch == Some(true))
        );
        assert!(marked.iter().all(|item| item.in_upstream == Some(false)));
    }

    #[test]
    fn handles_an_empty_history() {
        assert_eq!(mark_branch_membership(vec![]), []);
    }

    #[test]
    fn a_detached_head_has_no_upstream_to_pull_from() {
        let marked = mark_branch_membership(vec![
            commit("remoto", "aqui", vec![remote("origin/main")]),
            commit("aqui", "", vec![commit_ref("HEAD", RefType::Head, true)]),
        ]);

        assert_eq!(membership(&marked, "remoto"), (Some(false), Some(false)));
        assert_eq!(membership(&marked, "aqui"), (Some(true), Some(false)));
    }

    #[test]
    fn matches_the_upstream_of_a_branch_with_slashes() {
        let marked = mark_branch_membership(vec![
            commit("nuevo", "mio", vec![remote("origin/feature/login")]),
            commit("mio", "", vec![local_head("feature/login")]),
        ]);

        assert_eq!(membership(&marked, "nuevo"), (Some(false), Some(true)));
    }

    // --- groupRefBadges ---

    fn local(name: &str, is_head: bool) -> CommitRef {
        commit_ref(name, RefType::Local, is_head)
    }

    fn tag(name: &str) -> CommitRef {
        commit_ref(name, RefType::Tag, false)
    }

    fn remotes_of(badge: &RefBadge) -> Option<Vec<&str>> {
        badge
            .remotes
            .as_ref()
            .map(|remotes| remotes.iter().map(String::as_str).collect())
    }

    #[test]
    fn folds_a_remote_copy_into_its_local_branch_as_a_cloud() {
        let badges = group_ref_badges(&[local("main", true), tag("v1"), remote("origin/main")]);

        assert_eq!(badges.len(), 2);
        assert_eq!(
            (badges[0].name.as_str(), badges[0].kind),
            ("main", RefType::Local)
        );
        assert_eq!(remotes_of(&badges[0]), Some(vec!["origin"]));
        assert_eq!(
            (badges[1].name.as_str(), badges[1].kind),
            ("v1", RefType::Tag)
        );
        assert_eq!(badges[1].remotes, None);
    }

    #[test]
    fn keeps_one_cloud_per_remote_and_leaves_other_remotes_whole() {
        let badges = group_ref_badges(&[
            local("main", false),
            remote("origin/main"),
            remote("upstream/main"),
            remote("origin/develop"),
        ]);

        assert_eq!(badges.len(), 2);
        assert_eq!(badges[0].name, "main");
        assert_eq!(remotes_of(&badges[0]), Some(vec!["origin", "upstream"]));
        assert_eq!(badges[1].name, "origin/develop");
        assert_eq!(badges[1].remotes, None);
    }

    // `upstream/feature/main` no es la copia remota de `main`.
    #[test]
    fn does_not_mistake_a_nested_remote_branch_for_a_copy() {
        let badges = group_ref_badges(&[local("main", false), remote("upstream/feature/main")]);

        assert_eq!(
            badges
                .iter()
                .map(|badge| badge.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "upstream/feature/main"]
        );
        assert_eq!(remotes_of(&badges[0]), Some(vec![]));
    }

    #[test]
    fn leaves_remotes_alone_when_no_local_branch_is_on_the_commit() {
        let badges = group_ref_badges(&[remote("origin/main")]);

        assert_eq!(badges, [RefBadge::new(&remote("origin/main"), None)]);
        assert_eq!(
            serde_json::to_value(&badges[0]).unwrap(),
            serde_json::json!({ "name": "origin/main", "type": "remote", "isHead": false, "color": "" })
        );
    }

    // --- fitRefBadges ---

    fn badge_names() -> Vec<&'static str> {
        vec!["a", "bb", "ccc", "dddd"]
    }

    fn measure(name: &&str) -> f64 {
        name.len() as f64 * 10.0
    }

    #[test]
    fn shows_everything_when_it_fits_or_the_width_is_unknown() {
        let badges = badge_names();
        let all = FittedBadges {
            visible: badges.clone(),
            hidden: 0,
        };

        assert_eq!(
            fit_ref_badges(&badges, 200.0, measure, FitRefBadgesOptions::default()),
            all
        );
        assert_eq!(
            fit_ref_badges(&badges, 0.0, measure, FitRefBadgesOptions::default()),
            all
        );
        assert_eq!(
            fit_ref_badges(&badges, f64::NAN, measure, FitRefBadgesOptions::default()),
            all
        );
    }

    #[test]
    fn leaves_room_for_the_more_chip_when_something_has_to_go() {
        let badges = badge_names();

        // 10 + 4 + 20 = 34 usados; el siguiente (30) más el chip (34) no cabe en 80.
        assert_eq!(
            fit_ref_badges(&badges, 80.0, measure, FitRefBadgesOptions::default()),
            FittedBadges {
                visible: vec!["a", "bb"],
                hidden: 2
            }
        );
    }

    #[test]
    fn never_hides_the_first_badge() {
        let badges = badge_names();

        assert_eq!(
            fit_ref_badges(&badges, 12.0, measure, FitRefBadgesOptions::default()),
            FittedBadges {
                visible: vec!["a"],
                hidden: 3
            }
        );
    }

    // --- lo que hacían CommitView.vue y CommitHistoryList.vue con todo esto ---

    fn raw(full_hash: &str, parents: &str, message: &str, date: &str, refs: &str) -> RawCommit {
        RawCommit {
            hash: full_hash.chars().take(7).collect(),
            full_hash: full_hash.to_string(),
            parents: parents.to_string(),
            message: message.to_string(),
            author: "Test User".to_string(),
            email: "test@example.com".to_string(),
            date: date.to_string(),
            body: String::new(),
            refs: refs.to_string(),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-03-10T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn normalizes_a_raw_commit_into_a_history_row() {
        let commit = RawCommit {
            body: "Longer explanation".to_string(),
            ..raw(
                "1111111111111111",
                "2222222222222222",
                "current branch commit",
                "2026-03-09T00:00:00.000Z",
                "HEAD -> refs/heads/main, refs/remotes/origin/main",
            )
        };

        let row = normalize_raw_commit_at(&commit, "develop", &now());

        assert_eq!(row.hash, "1111111");
        assert_eq!(row.full_hash, "1111111111111111");
        assert_eq!(names(&row.refs), ["main", "origin/main"]);
        assert_eq!(row.message_body, "Longer explanation");
        assert_eq!(row.date, "1 day ago");
        assert_eq!(row.full_date, "2026-03-09T00:00:00.000Z");
        // El color es el de la rama local que apunta al commit...
        assert_eq!(row.color, get_branch_color("main"));
        assert_eq!(row.parents, "2222222222222222");
        assert_eq!(
            (row.graph.as_ref(), row.in_current_branch, row.in_upstream),
            (None, None, None)
        );

        // ...y, si ninguna apunta, el de la rama actual de la vista.
        let plain = normalize_raw_commit_at(
            &raw(
                "3333333333333333",
                "",
                "older",
                "2026-03-08T00:00:00.000Z",
                "",
            ),
            "develop",
            &now(),
        );
        assert_eq!(plain.color, get_branch_color("develop"));
        assert_eq!(plain.refs, []);
        assert_eq!(normalize_commit_view_branch(None), "HEAD");
        assert_eq!(normalize_commit_view_branch(Some("")), "HEAD");
        assert_eq!(normalize_commit_view_branch(Some("main")), "main");
    }

    // CommitView.test.js: "dims what is not in your branch and flags what you
    // have yet to pull", sobre los mismos commits.
    #[test]
    fn dims_what_is_not_in_your_branch_and_flags_what_you_have_yet_to_pull() {
        let rows: Vec<CommitRow> = [
            raw(
                "aaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbb",
                "waiting on the remote",
                "2026-03-10T00:00:00.000Z",
                "origin/main",
            ),
            raw(
                "cccccccccccccccc",
                "dddddddddddddddd",
                "from another branch",
                "2026-03-09T12:00:00.000Z",
                "feature",
            ),
            raw(
                "bbbbbbbbbbbbbbbb",
                "dddddddddddddddd",
                "current branch commit",
                "2026-03-09T00:00:00.000Z",
                "HEAD -> main",
            ),
            raw(
                "dddddddddddddddd",
                "",
                "shared root",
                "2026-03-08T00:00:00.000Z",
                "",
            ),
        ]
        .iter()
        .map(|commit| normalize_raw_commit_at(commit, "main", &now()))
        .collect();

        let rows = mark_branch_membership(build_commit_graph(rows));
        let by_message =
            |message: &str| rows.iter().find(|row| row.message == message).expect("row");
        let title = |message: &str| {
            let row = by_message(message);
            membership_title(row.in_current_branch, row.in_upstream)
        };

        assert_eq!(
            by_message("current branch commit").in_current_branch,
            Some(true)
        );
        assert_eq!(by_message("shared root").in_current_branch, Some(true));
        assert_eq!(
            by_message("from another branch").in_current_branch,
            Some(false)
        );
        assert_eq!(title("current branch commit"), None);
        assert_eq!(
            title("from another branch"),
            Some("Not in your current branch")
        );

        // Lo que falta por pullear también se apaga, pero además se marca: es un pull.
        let pending = by_message("waiting on the remote");
        assert_eq!(
            (pending.in_current_branch, pending.in_upstream),
            (Some(false), Some(true))
        );
        assert_eq!(
            title("waiting on the remote"),
            Some("Already on your upstream — pull to get it")
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.in_upstream == Some(true))
                .count(),
            1
        );

        // Y el grafo de esas mismas filas: nunca hay más de dos carriles a la vez.
        assert_eq!(graph_lane_count(&rows), 2);
        assert!(rows.iter().all(|row| row.graph.is_some()));
        assert!(is_head_commit(by_message("current branch commit")));
        assert!(!is_head_commit(by_message("shared root")));
    }

    // CommitView.test.js: "appends the next page of history without duplicates":
    // el grafo y la pertenencia se recalculan sobre la lista completa y las
    // filas ya visibles no cambian de carril ni de color.
    #[test]
    fn recomputing_over_a_longer_list_leaves_the_rows_already_shown_untouched() {
        let page = |range: std::ops::Range<u32>| -> Vec<CommitRow> {
            range
                .map(|index| {
                    let refs = if index == 1 { "HEAD -> main" } else { "" };
                    let hash = format!("{index:016}");
                    let parent = format!("{:016}", index + 1);
                    normalize_raw_commit_at(
                        &raw(
                            &hash,
                            &parent,
                            &format!("commit {index}"),
                            "2026-03-09T00:00:00.000Z",
                            refs,
                        ),
                        "main",
                        &now(),
                    )
                })
                .collect()
        };

        let first = mark_branch_membership(build_commit_graph(page(1..201)));
        let mut merged = first.clone();
        merged.extend(page(201..203));
        let merged = mark_branch_membership(build_commit_graph(merged));

        assert_eq!(merged.len(), 202);
        assert_eq!(merged[..200], first[..]);
        assert_eq!(graph_lane_count(&merged), 1);
    }

    #[test]
    fn falls_back_to_a_single_lane_for_a_commit_without_graph() {
        let fallback = CommitGraph::fallback("#409eff");

        assert_eq!(fallback.lane_count, 1);
        assert_eq!(fallback.color, "#409eff");
        assert_eq!(fallback.lanes[0].color, "#409eff");
        assert!(fallback.edges.is_empty());
        assert_eq!(CommitGraph::fallback("").lanes[0].color, "var(--primary)");
        assert_eq!(graph_lane_count(&[row("a", "a", "")]), 1);
        assert_eq!(graph_lane_count(&[]), 1);
    }

    #[test]
    fn graph_colors_are_borrowed_from_the_palette_and_serialize_as_text() {
        let commits = build_commit_graph(vec![
            row("m", "m", "a b"),
            row("b", "b", "a"),
            row("a", "a", ""),
        ]);
        for commit in &commits {
            let graph = graph(commit);
            assert!(matches!(graph.color, Cow::Borrowed(_)), "{graph:?}");
            assert!(
                graph
                    .lanes
                    .iter()
                    .all(|lane| matches!(lane.color, Cow::Borrowed(_)))
            );
            assert!(
                graph
                    .edges
                    .iter()
                    .all(|edge| matches!(edge.color, Cow::Borrowed(_)))
            );
        }

        let graph = graph(&commits[0]).clone();
        let json = serde_json::to_value(&graph).unwrap();
        assert_eq!(json["color"], serde_json::json!(get_graph_color(0)));
        let back: CommitGraph = serde_json::from_value(json).unwrap();
        assert_eq!(back, graph);
    }

    // CommitHistoryList.test.js: los títulos de la marca de HEAD.
    #[test]
    fn explains_where_head_is_and_which_remote_copies_a_badge_stands_for() {
        let head_badges = group_ref_badges(&[
            local("main", true),
            remote("origin/main"),
            remote("upstream/main"),
        ]);
        assert_eq!(
            badge_title(&head_badges[0]).as_deref(),
            Some("You are here · HEAD is on main · In sync with origin/main, upstream/main")
        );

        let detached = group_ref_badges(&[commit_ref("HEAD", RefType::Head, true)]);
        assert_eq!(
            badge_title(&detached[0]).as_deref(),
            Some("You are here · HEAD detached at this commit")
        );

        let plain = group_ref_badges(&[local("feature", false), tag("v1")]);
        assert_eq!(badge_title(&plain[0]), None);
        assert_eq!(badge_title(&plain[1]), None);
    }

    #[test]
    fn lists_in_the_more_chip_what_did_not_fit() {
        let badges = group_ref_badges(&[
            local("main", true),
            local("release", false),
            remote("origin/release"),
            tag("v1"),
        ]);

        assert_eq!(
            hidden_badge_title(&badges, 1),
            "release · origin/release · v1"
        );
        assert_eq!(hidden_badge_title(&badges, 3), "");
    }
}
