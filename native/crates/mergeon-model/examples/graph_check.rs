//! Comprueba el grafo del historial contra un repositorio real.
//!
//! ```bash
//! cargo run -p mergeon-model --example graph_check -- <repo> [commits=1000]
//! ```
//!
//! Pide el historial como la app (`git log --all --date-order`), calcula el
//! grafo con [`compute_commit_graph`] y lo recorre fila por fila mirando solo
//! lo que se pinta: qué carriles llegan desde arriba, cuáles siguen hasta la
//! fila siguiente y qué curvas salen del punto del commit. Con eso reconstruye
//! qué commit "lleva" cada línea y lo compara con los padres reales:
//!
//! - toda línea que llega a una fila viene de la fila anterior, y viceversa;
//! - la línea que llega al punto de un commit es la de ese commit, y si el
//!   commit tiene hijos cargados alguna línea llega a él;
//! - de cada commit sale una línea a cada uno de sus padres;
//! - ninguna línea termina a media fila sin llegar a un punto (salvo que se
//!   una con una curva a otra que va al mismo commit).
//!
//! Además cuenta en qué columnas va la línea de HEAD (HEAD y sus primeros
//! padres): lo esperable es una sola, la 0.
//!
//! Termina con código 1 si encuentra algún problema.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::process::Command;

use mergeon_model::commit_view_utils::{
    CommitRow, GraphEdgeType, compute_commit_graph, is_head_commit, parse_refs,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let repo = args.next().expect("uso: graph_check <repo> [commits]");
    let max_count: usize = args.next().map_or(1000, |n| n.parse().expect("commits"));

    let output = Command::new("git")
        .args([
            "-C",
            &repo,
            "log",
            "--all",
            "--date-order",
            "--decorate=full",
            &format!("--max-count={max_count}"),
            "--format=%H%x1f%P%x1f%s%x1f%D",
        ])
        .output()
        .expect("git log");
    let rows: Vec<CommitRow> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let mut fields = line.split('\u{1f}');
            let hash = fields.next().unwrap_or_default().to_string();
            CommitRow {
                hash: hash[..hash.len().min(8)].to_string(),
                full_hash: hash,
                parents: fields.next().unwrap_or_default().to_string(),
                message: fields.next().unwrap_or_default().to_string(),
                refs: parse_refs(fields.next().unwrap_or_default()),
                ..Default::default()
            }
        })
        .collect();
    let graphs = compute_commit_graph(&rows);

    // La línea de HEAD: HEAD y sus primeros padres.
    let by_hash: std::collections::HashMap<&str, &CommitRow> = rows
        .iter()
        .map(|row| (row.full_hash.as_str(), row))
        .collect();
    let mut head_line = HashSet::new();
    let mut cursor = rows.iter().find(|row| is_head_commit(*row));
    while let Some(row) = cursor {
        head_line.insert(row.full_hash.as_str());
        cursor = row
            .parents
            .split_whitespace()
            .next()
            .and_then(|parent| by_hash.get(parent).copied());
    }
    let mut head_columns: BTreeMap<usize, usize> = BTreeMap::new();

    let mut with_children: HashSet<&str> = HashSet::new();
    let mut problems = Vec::new();
    let mut carry: BTreeMap<usize, String> = BTreeMap::new();
    let (mut edges, mut crossings, mut max_lanes) = (0usize, 0usize, 0usize);

    for (row_index, (row, graph)) in rows.iter().zip(&graphs).enumerate() {
        let mut report = |text: String| {
            problems.push(format!(
                "fila {row_index} {} «{}»: {text}",
                row.hash, row.message
            ))
        };
        let hash = row.full_hash.as_str();
        let parents: Vec<&str> = row.parents.split_whitespace().collect();
        let current = graph.lane;
        if head_line.contains(hash) {
            *head_columns.entry(current).or_default() += 1;
        }
        max_lanes = max_lanes.max(graph.lane_count);

        let arriving: BTreeSet<usize> = graph
            .lanes
            .iter()
            .filter(|lane| lane.arrives)
            .map(|lane| lane.index)
            .collect();
        let carried: BTreeSet<usize> = carry.keys().copied().collect();
        if arriving != carried {
            report(format!(
                "llegan carriles {arriving:?} pero la fila anterior dejó {carried:?}"
            ));
        }
        match carry.get(&current) {
            Some(carried_hash) if carried_hash != hash => report(format!(
                "la línea que llega al punto es de {}",
                &carried_hash[..8]
            )),
            None if with_children.contains(hash) => {
                report("tiene hijos cargados pero ninguna línea llega a su punto".into())
            }
            _ => {}
        }
        for (lane, carried_hash) in &carry {
            if *lane != current && carried_hash == hash {
                report(format!(
                    "otra línea (carril {lane}) también va a este commit"
                ));
            }
        }

        let joined: HashSet<usize> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == GraphEdgeType::LaneShift)
            .map(|edge| edge.from)
            .collect();
        let mut next: BTreeMap<usize, String> = BTreeMap::new();
        for lane in &graph.lanes {
            if lane.index == current {
                if lane.continues {
                    report("el carril del commit sigue recto sin ser una arista".into());
                }
                continue;
            }
            match (lane.continues, carry.get(&lane.index)) {
                (true, Some(carried_hash)) => {
                    next.insert(lane.index, carried_hash.clone());
                }
                (true, None) => report(format!("el carril {} sigue sin llegar", lane.index)),
                (false, Some(_)) if lane.arrives && !joined.contains(&lane.index) => {
                    report(format!(
                        "el carril {} termina a media fila sin llegar a un punto",
                        lane.index
                    ))
                }
                _ => {}
            }
        }

        let mut pending_parents = parents.iter().copied();
        let mut reached = HashSet::new();
        for edge in &graph.edges {
            edges += 1;
            let (low, high) = (edge.from.min(edge.to), edge.from.max(edge.to));
            crossings += graph
                .lanes
                .iter()
                .filter(|lane| lane.continues && lane.index > low && lane.index < high)
                .count();
            match edge.kind {
                GraphEdgeType::LaneShift => {
                    let Some(carried_hash) = carry.get(&edge.from) else {
                        report(format!(
                            "desplazamiento desde el carril vacío {}",
                            edge.from
                        ));
                        continue;
                    };
                    match next.get(&edge.to) {
                        Some(existing) if existing != carried_hash => report(format!(
                            "la unión desde el carril {} cae en el {} que lleva otro commit",
                            edge.from, edge.to
                        )),
                        _ => {
                            next.insert(edge.to, carried_hash.clone());
                        }
                    }
                }
                GraphEdgeType::Parent | GraphEdgeType::Merge => {
                    if edge.from != current {
                        report(format!("arista que no sale del punto ({})", edge.from));
                    }
                    let Some(parent) = pending_parents.next() else {
                        report("más aristas que padres".into());
                        continue;
                    };
                    reached.insert(parent);
                    match next.get(&edge.to) {
                        Some(existing) if existing != parent => report(format!(
                            "la arista a {} cae en el carril {} que lleva {}",
                            &parent[..8],
                            edge.to,
                            &existing[..8]
                        )),
                        _ => {
                            next.insert(edge.to, parent.to_string());
                        }
                    }
                }
            }
        }
        for parent in &parents {
            if !reached.contains(parent) {
                report(format!("ninguna línea va a su padre {}", &parent[..8]));
            }
            with_children.insert(parent);
        }
        carry = next;
    }

    println!(
        "{repo}: {} commits, {} merges, {edges} aristas, {crossings} cruces de aristas sobre carriles, {max_lanes} carriles como máximo",
        rows.len(),
        rows.iter()
            .filter(|row| row.parents.split_whitespace().count() > 1)
            .count(),
    );
    println!("  línea de HEAD por columna (columna: commits): {head_columns:?}");
    if problems.is_empty() {
        println!("  sin problemas");
    } else {
        println!("  {} problemas; los primeros:", problems.len());
        for problem in problems.iter().take(15) {
            println!("  - {problem}");
        }
        std::process::exit(1);
    }
}
