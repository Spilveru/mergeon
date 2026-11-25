//! Deshacer y rehacer operaciones de git (propio, sin original).
//!
//! Cada repositorio tiene su pila (por ruta, en el modelo y no en una vista).
//! Las mutaciones que se pueden deshacer la alimentan solas: su canal del
//! backend lee HEAD, la rama y los refs que toca antes y después de correr,
//! dentro de su turno en la cola (`Mutation::capture`), y aquí eso se
//! convierte en una entrada con los pasos de ida y vuelta
//! ([`UndoStep`]). Deshacer y rehacer son a su vez mutaciones (`undo` /
//! `redo`) que pasan por la misma cola, salen en el log de operaciones y
//! relean el repositorio por los caminos de siempre.
//!
//! | Operación | Deshacer | Rehacer |
//! |---|---|---|
//! | commit, amend, fixup | `reset --soft` al HEAD anterior (los cambios vuelven al índice) | `reset --soft` al commit |
//! | commit que cierra un merge | `reset --keep` | `reset --keep` |
//! | merge, rebase (también interactivo / autosquash, reword, drop…), pull, cherry-pick, revert, fast-forward | `reset --keep` (se niega antes de pisar cambios locales) | `reset --keep` |
//! | reset `soft` / `mixed` / `hard` | `reset --soft` / `--mixed` / `--keep` de vuelta | lo mismo hacia delante |
//! | checkout de rama, commit o tag | checkout de vuelta (sin `-f`) | checkout |
//! | checkout de rama remota | checkout de vuelta y borra la rama creada | la recrea con su upstream y checkout |
//! | crear / renombrar / borrar rama | borrar (si sigue igual) / renombrar al revés / recrear con su hash y upstream | al revés |
//! | crear / borrar tag | `update-ref` (un anotado vuelve con su mismo objeto tag) | al revés |
//! | stash push / pop / apply / drop | pop / quita lo aplicado y `stash store` / quita lo aplicado / `stash store` | de nuevo |
//! | descartar archivo, todo, bloque o líneas | parche y copias guardadas antes (ver abajo) | vuelve a descartar si nada cambió |
//!
//! Push, fetch, borrar ramas o tags remotos, limpieza de ramas, worktrees y
//! submódulos no entran en la pila ni la invalidan.
//!
//! Antes de deshacer, cada paso comprueba que el repositorio sigue como lo
//! dejó la operación (HEAD y rama, el ref con el mismo valor, el stash en la
//! lista…). Si alguien lo cambió por fuera (un commit o un checkout en la
//! terminal), la entrada ya no aplica: la pila del repositorio se vacía y se
//! avisa. Si deshacer quitaría commits que ya están en el upstream de la
//! rama, se pide confirmación (haría falta un force push; Mergeon nunca hace
//! push por su cuenta).
//!
//! **Descartes.** Antes de descartar, en el mismo turno de la cola, se guarda
//! en `<datos de la app>/undo-backups/<sesión>/<n>/` el parche índice →
//! working tree de los archivos con seguimiento (`git diff --binary`, que
//! cubre binarios) y una copia byte a byte de lo que no tiene seguimiento.
//! Se eligió esto y no `git stash create` + `store`: el stash no guarda lo
//! que no tiene seguimiento sin tocar el working tree y aparecería en la
//! lista de stashes. Deshacer aplica el parche (`git apply --check` antes: si
//! el archivo cambió, no toca nada) y restaura las copias donde ya no hay
//! nada. Por encima de [`DISCARD_BACKUP_LIMIT`] no se guarda copia: se pide
//! confirmación antes de descartar sin poder deshacer. Las copias se borran
//! al salir de la pila y al cerrar la app; las de sesiones viejas, al
//! arrancar.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui_kit::{AppContext as _, Context, Task};
use mergeon_backend::undo::{
    DiscardBackupOutcome, HeadTarget, PatchData, RefCapture, RefSnapshot, ResetMode, UndoStep,
};
use mergeon_backend::{FailureCode, MutationBody, Outcome, create_generic_operation_failure};
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::operation_types::OperationResult as OperationSummary;

use crate::AppModel;
use crate::git_client::GitMutation;
use crate::repository_sync::SyncRequest;
use crate::tasks::flow;

/// Entradas por repositorio como mucho (las más viejas se olvidan).
pub const UNDO_LIMIT: usize = 50;

/// Tamaño máximo de la copia de un descarte (50 MiB).
pub const DISCARD_BACKUP_LIMIT: u64 = 50 * 1024 * 1024;

/// Las copias de sesiones anteriores que tengan más de esto se borran al
/// arrancar.
const STALE_BACKUP_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Una operación que se puede deshacer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    /// Qué se hizo: "commit 'Fix login'", "merge of feature/x".
    pub label: String,
    pub undo: Vec<UndoStep>,
    pub redo: Vec<UndoStep>,
    /// Carpeta con la copia de un descarte (se borra cuando la entrada sale).
    pub backup: Option<PathBuf>,
    /// Preguntar antes si deshacer quitaría commits que ya están en el
    /// upstream. No en un pull o un fast-forward: lo que quitan vino del
    /// upstream y la rama solo vuelve a quedar por detrás.
    pub confirm_published: bool,
    /// Si deshacer esta entrada debe devolver el mensaje a la caja de commit
    /// (cuando esté vacía).
    pub restore_commit_message: Option<String>,
    /// Aviso adicional para el toast tras deshacer (p. ej. reset hard con
    /// cambios sin confirmar que no vuelven).
    pub undo_note: Option<String>,
}

impl UndoEntry {
    fn simple(label: String, undo: Vec<UndoStep>, redo: Vec<UndoStep>) -> Self {
        Self {
            label,
            undo,
            redo,
            backup: None,
            confirm_published: true,
            restore_commit_message: None,
            undo_note: None,
        }
    }
}

/// Pila de deshacer y rehacer con tope. Sin git: solo el orden.
#[derive(Debug, Clone)]
pub struct UndoStack<E> {
    undo: VecDeque<E>,
    redo: Vec<E>,
    limit: usize,
}

impl<E> Default for UndoStack<E> {
    fn default() -> Self {
        Self::new(UNDO_LIMIT)
    }
}

impl<E> UndoStack<E> {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Una operación nueva: va arriba de deshacer y vacía rehacer. Devuelve
    /// lo que sale (rehacer y, pasado el tope, lo más viejo).
    pub fn record(&mut self, entry: E) -> Vec<E> {
        let mut dropped: Vec<E> = std::mem::take(&mut self.redo);
        self.undo.push_back(entry);
        dropped.extend(self.trim());
        dropped
    }

    fn trim(&mut self) -> Vec<E> {
        let mut dropped = Vec::new();
        while self.undo.len() > self.limit {
            dropped.extend(self.undo.pop_front());
        }
        dropped
    }

    /// Lo próximo que se desharía (o, con `redo`, se reharía).
    pub fn peek(&self, redo: bool) -> Option<&E> {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
    }

    /// Saca lo próximo para deshacerlo o rehacerlo.
    pub fn take(&mut self, redo: bool) -> Option<E> {
        if redo {
            self.redo.pop()
        } else {
            self.undo.pop_back()
        }
    }

    /// No se pudo: vuelve a donde estaba.
    pub fn put_back(&mut self, redo: bool, entry: E) {
        if redo {
            self.redo.push(entry);
        } else {
            self.undo.push_back(entry);
        }
    }

    /// Se deshizo (o rehizo): pasa a la otra pila. Devuelve lo que sale por
    /// el tope.
    pub fn finish(&mut self, redo: bool, entry: E) -> Vec<E> {
        if redo {
            self.undo.push_back(entry);
            self.trim()
        } else {
            self.redo.push(entry);
            let mut dropped = Vec::new();
            while self.redo.len() > self.limit {
                dropped.push(self.redo.remove(0));
            }
            dropped
        }
    }

    /// Vacía las dos pilas y devuelve lo que había.
    pub fn clear(&mut self) -> Vec<E> {
        let mut dropped: Vec<E> = self.undo.drain(..).collect();
        dropped.append(&mut self.redo);
        dropped
    }

    pub fn len(&self, redo: bool) -> usize {
        if redo {
            self.redo.len()
        } else {
            self.undo.len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }

    /// De la más reciente a la más vieja.
    pub fn iter(&self, redo: bool) -> Box<dyn Iterator<Item = &E> + '_> {
        if redo {
            Box::new(self.redo.iter().rev())
        } else {
            Box::new(self.undo.iter().rev())
        }
    }
}

/// Un merge, rebase, cherry-pick… que se detuvo con conflictos: cuando un
/// commit o "Continue" lo termine, la entrada vuelve a donde estaba HEAD
/// antes de empezar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSequence {
    pub label: String,
    pub branch: Option<String>,
    pub before: String,
}

#[derive(Debug, Default)]
struct RepoUndo {
    stack: UndoStack<UndoEntry>,
    pending: Option<PendingSequence>,
}

/// Estado de deshacer de la app (en [`AppModel`]).
#[derive(Debug, Default)]
pub(crate) struct UndoState {
    repos: HashMap<String, RepoUndo>,
    /// Repositorios con un deshacer / rehacer en vuelo.
    running: HashSet<String>,
    /// `<datos>/undo-backups/<sesión>` (se crea al primer descarte).
    backup_root: Option<PathBuf>,
    next_backup: u64,
}

/// Lo que una mutación sabe de sí misma para poder deshacerse (lo pone cada
/// método de `git_client`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoIntent {
    Commit {
        message: String,
        amend: bool,
        fixup: bool,
    },
    /// Mueve HEAD sin conflictos posibles: reset.
    HeadMove {
        label: String,
        mode: ResetMode,
        /// `reset --hard` con cambios sin confirmar en archivos rastreados:
        /// deshacer devuelve HEAD, pero esos cambios descartados no vuelven.
        discarded_changes: bool,
    },
    /// Merge, rebase, cherry-pick, revert, rebase interactivo y sus
    /// acciones: pueden quedarse a medias con conflictos.
    Sequence {
        label: String,
    },
    /// Pull o fast-forward: como `Sequence`, pero lo que trae ya está en el
    /// upstream (deshacerlo no pide confirmación).
    Pull {
        label: String,
    },
    /// Continue / skip / abort de la operación en curso.
    SequenceAction {
        action: String,
    },
    Checkout {
        label: String,
    },
    CheckoutRemote {
        remote_branch: String,
    },
    CreateBranch {
        name: String,
    },
    RenameBranch {
        from: String,
        to: String,
    },
    DeleteBranch {
        name: String,
    },
    CreateTag {
        name: String,
    },
    DeleteTag {
        name: String,
    },
    StashPush,
    StashApply {
        stash_ref: String,
        pop: bool,
    },
    StashDrop {
        stash_ref: String,
    },
    /// Descartar archivos (la copia viene en la captura).
    Discard {
        label: String,
    },
    /// Descartar un bloque o unas líneas: el parche es la copia.
    DiscardPatch {
        label: String,
        patch: String,
        reverse: bool,
    },
}

/// Qué hacer con la operación pendiente de un repositorio.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PendingChange {
    #[default]
    Keep,
    Set(PendingSequence),
    Clear,
}

/// Lo que deja una operación terminada en la pila.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UndoPlan {
    pub entry: Option<UndoEntry>,
    pub pending: PendingChange,
    /// Aviso para quien la hizo (la copia del descarte no se pudo guardar).
    pub warning: Option<String>,
}

/// Primera línea de un mensaje de commit, recortada para una etiqueta.
pub fn commit_subject(message: &str) -> String {
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    const MAX: usize = 50;
    if line.chars().count() > MAX {
        let cut: String = line.chars().take(MAX - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn in_progress(summary: &OperationSummary) -> Option<bool> {
    summary
        .status
        .as_ref()
        .map(|status| status.operation_state.in_progress)
}

fn conflicted(summary: &OperationSummary) -> bool {
    summary.outcome.as_deref() == Some("conflicted")
        || summary.code.as_deref() == Some("CONFLICT")
        || summary.code.as_deref() == Some("STASH_CONFLICT")
}

/// Una sequencia de commit en curso (merge, cherry-pick o revert a medias)
/// antes de la operación.
const SEQUENCE_HEADS: [&str; 3] = ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"];

fn head_move(
    label: String,
    branch: Option<&str>,
    before: &str,
    after: &str,
    mode: ResetMode,
) -> UndoEntry {
    let branch = branch.map(str::to_string);
    UndoEntry::simple(
        label,
        vec![UndoStep::MoveHead {
            branch: branch.clone(),
            from: after.to_string(),
            to: before.to_string(),
            mode,
        }],
        vec![UndoStep::MoveHead {
            branch,
            from: before.to_string(),
            to: after.to_string(),
            mode,
        }],
    )
}

/// HEAD se movió en la misma rama (o desanclado) de `before` a `after`.
fn moved(capture: &RefCapture) -> Option<(&str, &str)> {
    let before = capture.before.head.as_deref()?;
    let after = capture.after.head.as_deref()?;
    (before != after && capture.before.head_ref == capture.after.head_ref)
        .then_some((before, after))
}

/// Merge / rebase con rama destino: la operación cambió antes a otra rama
/// (la única `refs/heads/...` capturada) y esa rama estaba en este commit.
fn switched_branch(capture: &RefCapture) -> Option<(&str, &str)> {
    let (reference, before) = capture
        .before
        .refs
        .iter()
        .find(|(reference, _)| reference.starts_with("refs/heads/"))?;
    let branch = reference.strip_prefix("refs/heads/")?;
    (capture.before.branch() != Some(branch)).then_some((branch, before.as_str()))
}

/// Entrada de una secuencia que cambió a `target` y la movió: deshacer
/// devuelve `target` a donde estaba y vuelve a la rama de antes.
fn switched_sequence_entry(
    label: &str,
    capture: &RefCapture,
    target: &str,
    target_before: &str,
) -> Option<UndoEntry> {
    let after = capture.after.head.as_deref()?;
    let origin = capture.before.head_target()?;
    let target_head = HeadTarget::Branch(target.to_string());
    let mut undo = Vec::new();
    let mut redo = vec![UndoStep::Checkout {
        from: origin.clone(),
        to: target_head.clone(),
    }];
    if target_before != after {
        undo.push(UndoStep::MoveHead {
            branch: Some(target.to_string()),
            from: after.to_string(),
            to: target_before.to_string(),
            mode: ResetMode::Keep,
        });
        redo.push(UndoStep::MoveHead {
            branch: Some(target.to_string()),
            from: target_before.to_string(),
            to: after.to_string(),
            mode: ResetMode::Keep,
        });
    }
    undo.push(UndoStep::Checkout {
        from: target_head,
        to: origin,
    });
    Some(UndoEntry::simple(label.to_string(), undo, redo))
}

fn upstream_of(snapshot: &RefSnapshot, branch: &str) -> Option<(String, String)> {
    let remote = snapshot.config(&format!("branch.{branch}.remote"))?;
    let merge = snapshot.config(&format!("branch.{branch}.merge"))?;
    Some((remote.to_string(), merge.to_string()))
}

/// Convierte lo que se sabe de una operación terminada en lo que deja en la
/// pila. Pura: los tests la prueban sin git.
pub fn plan_undo(
    intent: &UndoIntent,
    capture: &RefCapture,
    summary: &OperationSummary,
    pending: Option<&PendingSequence>,
) -> UndoPlan {
    let mut plan = UndoPlan::default();
    let completed = summary.success && summary.outcome.as_deref() != Some("noop");
    match intent {
        UndoIntent::Commit {
            message,
            amend,
            fixup,
        } => {
            if !completed {
                return plan;
            }
            let Some((before, after)) = moved(capture) else {
                return plan;
            };
            let branch = capture.after.branch();
            let concluding = SEQUENCE_HEADS
                .iter()
                .any(|head| capture.before.rev(head).is_some());
            if concluding {
                plan.pending = PendingChange::Clear;
                // El commit que cierra un merge, cherry-pick o revert detenido
                // en la app deshace la operación entera.
                if let Some(pending) = pending.filter(|pending| pending.branch.as_deref() == branch)
                {
                    plan.entry = Some(head_move(
                        pending.label.clone(),
                        branch,
                        &pending.before,
                        after,
                        ResetMode::Keep,
                    ));
                    return plan;
                }
                let label = if capture.before.rev("MERGE_HEAD").is_some() {
                    format!("merge commit '{}'", commit_subject(message))
                } else {
                    format!("commit '{}'", commit_subject(message))
                };
                plan.entry = Some(head_move(label, branch, before, after, ResetMode::Keep));
                return plan;
            }
            let subject = commit_subject(message);
            let label = if *fixup {
                "fixup commit".to_string()
            } else if *amend {
                format!("amend of '{subject}'")
            } else {
                format!("commit '{subject}'")
            };
            let mut entry = head_move(label, branch, before, after, ResetMode::Soft);
            if !*amend && !*fixup && !message.trim().is_empty() {
                entry.restore_commit_message = Some(message.clone());
            }
            plan.entry = Some(entry);
        }
        UndoIntent::HeadMove {
            label,
            mode,
            discarded_changes,
        } => {
            if let (true, Some((before, after))) = (completed, moved(capture)) {
                let mut entry =
                    head_move(label.clone(), capture.after.branch(), before, after, *mode);
                if *discarded_changes {
                    entry.undo_note = Some(
                        "Uncommitted changes discarded by the hard reset were not restored."
                            .to_string(),
                    );
                }
                plan.entry = Some(entry);
            }
        }
        UndoIntent::Sequence { label } | UndoIntent::Pull { label } => {
            let switched = switched_branch(capture);
            if completed {
                plan.pending = PendingChange::Clear;
                if let Some((before, after)) = moved(capture) {
                    plan.entry = Some(head_move(
                        label.clone(),
                        capture.after.branch(),
                        before,
                        after,
                        ResetMode::Keep,
                    ));
                } else if let Some((target, target_before)) = switched
                    && capture.after.branch() == Some(target)
                {
                    plan.entry = switched_sequence_entry(label, capture, target, target_before);
                }
            } else if conflicted(summary) && in_progress(summary) != Some(false) {
                // Si la operación cambió antes a otra rama, lo pendiente es
                // esa rama (un rebase a medias deja HEAD desanclado).
                let pending = match switched {
                    Some((target, target_before)) => {
                        Some((Some(target.to_string()), target_before))
                    }
                    None => capture
                        .before
                        .head
                        .as_deref()
                        .map(|before| (capture.before.branch().map(str::to_string), before)),
                };
                if let Some((branch, before)) = pending {
                    plan.pending = PendingChange::Set(PendingSequence {
                        label: label.clone(),
                        branch,
                        before: before.to_string(),
                    });
                }
            }
        }
        UndoIntent::SequenceAction { action } => {
            if !summary.success {
                return plan;
            }
            if action == "abort" {
                plan.pending = PendingChange::Clear;
                return plan;
            }
            if in_progress(summary) != Some(false) {
                return plan;
            }
            plan.pending = PendingChange::Clear;
            if let (Some(pending), Some(after)) = (pending, capture.after.head.as_deref())
                && pending.branch.as_deref() == capture.after.branch()
                && pending.before != after
            {
                plan.entry = Some(head_move(
                    pending.label.clone(),
                    capture.after.branch(),
                    &pending.before,
                    after,
                    ResetMode::Keep,
                ));
            }
        }
        UndoIntent::Checkout { label } => {
            if !completed {
                return plan;
            }
            let (Some(from), Some(to)) =
                (capture.before.head_target(), capture.after.head_target())
            else {
                return plan;
            };
            if from != to {
                plan.entry = Some(UndoEntry {
                    label: format!("checkout of '{label}'"),
                    undo: vec![UndoStep::Checkout {
                        from: to.clone(),
                        to: from.clone(),
                    }],
                    redo: vec![UndoStep::Checkout { from, to }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::CheckoutRemote { remote_branch } => {
            if !completed {
                return plan;
            }
            let local = remote_branch
                .split_once('/')
                .map_or("", |(_, branch)| branch);
            let reference = format!("refs/heads/{local}");
            let (Some(from), Some(to), Some(sha)) = (
                capture.before.head_target(),
                capture.after.head_target(),
                capture.after.rev(&reference),
            ) else {
                return plan;
            };
            if capture.before.rev(&reference).is_some() {
                return plan;
            }
            plan.entry = Some(UndoEntry {
                label: format!("checkout of '{remote_branch}'"),
                undo: vec![
                    UndoStep::Checkout {
                        from: to.clone(),
                        to: from.clone(),
                    },
                    UndoStep::DeleteBranch {
                        name: local.to_string(),
                        sha: sha.to_string(),
                    },
                ],
                redo: vec![
                    UndoStep::CreateBranch {
                        name: local.to_string(),
                        sha: sha.to_string(),
                        upstream: upstream_of(&capture.after, local),
                    },
                    UndoStep::Checkout { from, to },
                ],
                backup: None,
                confirm_published: true,
                restore_commit_message: None,
                undo_note: None,
            });
        }
        UndoIntent::CreateBranch { name } => {
            let reference = format!("refs/heads/{name}");
            let (true, Some(sha), None) = (
                completed,
                capture.after.rev(&reference),
                capture.before.rev(&reference),
            ) else {
                return plan;
            };
            let create = UndoStep::CreateBranch {
                name: name.clone(),
                sha: sha.to_string(),
                upstream: None,
            };
            let delete = UndoStep::DeleteBranch {
                name: name.clone(),
                sha: sha.to_string(),
            };
            let (from, to) = (capture.before.head_target(), capture.after.head_target());
            let (undo, redo) = match (from, to) {
                (Some(from), Some(to)) if from != to => (
                    vec![
                        UndoStep::Checkout {
                            from: to.clone(),
                            to: from.clone(),
                        },
                        delete,
                    ],
                    vec![create, UndoStep::Checkout { from, to }],
                ),
                _ => (vec![delete], vec![create]),
            };
            plan.entry = Some(UndoEntry {
                label: format!("create branch '{name}'"),
                undo,
                redo,
                backup: None,
                confirm_published: true,
                restore_commit_message: None,
                undo_note: None,
            });
        }
        UndoIntent::RenameBranch { from, to } => {
            if completed {
                plan.entry = Some(UndoEntry {
                    label: format!("rename of branch '{from}' to '{to}'"),
                    undo: vec![UndoStep::RenameBranch {
                        from: to.clone(),
                        to: from.clone(),
                    }],
                    redo: vec![UndoStep::RenameBranch {
                        from: from.clone(),
                        to: to.clone(),
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::DeleteBranch { name } => {
            let reference = format!("refs/heads/{name}");
            if let (true, Some(sha), None) = (
                completed,
                capture.before.rev(&reference),
                capture.after.rev(&reference),
            ) {
                plan.entry = Some(UndoEntry {
                    label: format!("delete branch '{name}'"),
                    undo: vec![UndoStep::CreateBranch {
                        name: name.clone(),
                        sha: sha.to_string(),
                        upstream: upstream_of(&capture.before, name),
                    }],
                    redo: vec![UndoStep::DeleteBranch {
                        name: name.clone(),
                        sha: sha.to_string(),
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::CreateTag { name } => {
            let reference = format!("refs/tags/{name}");
            if let (true, Some(sha), None) = (
                completed,
                capture.after.rev(&reference),
                capture.before.rev(&reference),
            ) {
                plan.entry = Some(UndoEntry {
                    label: format!("create tag '{name}'"),
                    undo: vec![UndoStep::DeleteTag {
                        name: name.clone(),
                        sha: sha.to_string(),
                    }],
                    redo: vec![UndoStep::CreateTag {
                        name: name.clone(),
                        sha: sha.to_string(),
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::DeleteTag { name } => {
            let reference = format!("refs/tags/{name}");
            if let (true, Some(sha), None) = (
                completed,
                capture.before.rev(&reference),
                capture.after.rev(&reference),
            ) {
                plan.entry = Some(UndoEntry {
                    label: format!("delete tag '{name}'"),
                    undo: vec![UndoStep::CreateTag {
                        name: name.clone(),
                        sha: sha.to_string(),
                    }],
                    redo: vec![UndoStep::DeleteTag {
                        name: name.clone(),
                        sha: sha.to_string(),
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::StashPush => {
            let after = capture.after.rev("refs/stash");
            if let (true, Some(sha)) = (completed, after)
                && capture.before.rev("refs/stash") != Some(sha)
            {
                plan.entry = Some(UndoEntry {
                    label: "stash".to_string(),
                    undo: vec![UndoStep::StashApply {
                        sha: sha.to_string(),
                        pop: true,
                    }],
                    redo: vec![
                        UndoStep::StashUnapply {
                            sha: sha.to_string(),
                        },
                        UndoStep::StashStore {
                            sha: sha.to_string(),
                        },
                    ],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::StashApply { stash_ref, pop } => {
            if let (true, Some(sha)) = (completed, capture.before.rev(stash_ref)) {
                let sha = sha.to_string();
                let mut undo = vec![UndoStep::StashUnapply { sha: sha.clone() }];
                if *pop {
                    undo.push(UndoStep::StashStore { sha: sha.clone() });
                }
                plan.entry = Some(UndoEntry {
                    label: format!(
                        "{} of stash {}",
                        if *pop { "pop" } else { "apply" },
                        short_sha(&sha)
                    ),
                    undo,
                    redo: vec![UndoStep::StashApply { sha, pop: *pop }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::StashDrop { stash_ref } => {
            if let (true, Some(sha)) = (completed, capture.before.rev(stash_ref)) {
                plan.entry = Some(UndoEntry {
                    label: format!("drop of stash {}", short_sha(sha)),
                    undo: vec![UndoStep::StashStore {
                        sha: sha.to_string(),
                    }],
                    redo: vec![UndoStep::StashDrop {
                        sha: sha.to_string(),
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
        UndoIntent::Discard { label } => match &capture.discard {
            Some(DiscardBackupOutcome::Saved(backup)) if completed => {
                let mut undo = Vec::new();
                let mut redo = Vec::new();
                if let Some(patch) = &backup.patch {
                    undo.push(UndoStep::ApplyPatch {
                        patch: PatchData::File(patch.clone()),
                        reverse: false,
                    });
                }
                if !backup.untracked.is_empty() {
                    undo.push(UndoStep::RestoreFiles {
                        files: backup.untracked.clone(),
                    });
                    redo.push(UndoStep::RemoveFiles {
                        files: backup.untracked.clone(),
                    });
                }
                if let Some(patch) = &backup.patch {
                    redo.push(UndoStep::ApplyPatch {
                        patch: PatchData::File(patch.clone()),
                        reverse: true,
                    });
                }
                if !undo.is_empty() {
                    plan.entry = Some(UndoEntry {
                        label: label.clone(),
                        undo,
                        redo,
                        backup: Some(backup.directory.clone()),
                        confirm_published: true,
                        restore_commit_message: None,
                        undo_note: None,
                    });
                }
            }
            Some(DiscardBackupOutcome::TooLarge(bytes)) if completed => {
                plan.warning = Some(format!(
                    "The discarded changes ({}) were too large to keep an undo copy.",
                    format_bytes(*bytes)
                ));
            }
            Some(DiscardBackupOutcome::Failed(error)) if completed => {
                plan.warning = Some(format!(
                    "Couldn't keep an undo copy of the discarded changes: {error}"
                ));
            }
            _ => {}
        },
        UndoIntent::DiscardPatch {
            label,
            patch,
            reverse,
        } => {
            if completed {
                plan.entry = Some(UndoEntry {
                    label: label.clone(),
                    undo: vec![UndoStep::ApplyPatch {
                        patch: PatchData::Text(patch.clone()),
                        reverse: !reverse,
                    }],
                    redo: vec![UndoStep::ApplyPatch {
                        patch: PatchData::Text(patch.clone()),
                        reverse: *reverse,
                    }],
                    backup: None,
                    confirm_published: true,
                    restore_commit_message: None,
                    undo_note: None,
                });
            }
        }
    }
    plan
}

/// "50 MB", "1.5 MB", "820 KB".
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= 10.0 * MB {
        format!("{:.0} MB", value / MB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else {
        format!("{:.0} KB", (value / 1024.0).max(1.0))
    }
}

/// "Undo commit 'Fix login'" / "Redo commit 'Fix login'".
pub fn action_label(label: &str, redo: bool) -> String {
    format!("{} {label}", if redo { "Redo" } else { "Undo" })
}

/// Los movimientos de HEAD de unos pasos que podrían quitar commits
/// publicados: (rama, desde, hasta).
fn head_moves(steps: &[UndoStep]) -> Vec<(String, String, String)> {
    steps
        .iter()
        .filter_map(|step| match step {
            UndoStep::MoveHead {
                branch: Some(branch),
                from,
                to,
                ..
            } => Some((branch.clone(), from.clone(), to.clone())),
            _ => None,
        })
        .collect()
}

fn remove_dirs(dirs: Vec<PathBuf>, cx: &mut Context<AppModel>) {
    if dirs.is_empty() {
        return;
    }
    cx.background_spawn(async move {
        for dir in dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    })
    .detach();
}

/// Borra las carpetas de copias de otras sesiones con más de un día.
fn purge_stale_backups(root: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path == current {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_BACKUP_AGE);
        if old {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Un resultado "cancelado" sin llegar a la cola (el usuario dijo que no).
fn cancelled_mutation(operation: &str, message: &str) -> GitMutation {
    let mut failure =
        create_generic_operation_failure(operation, "", "", serde_json::Map::default());
    failure.outcome = Outcome::Cancelled;
    failure.code = FailureCode::Cancelled;
    failure.message = message.to_string();
    GitMutation {
        operation_id: None,
        body: MutationBody::Failed(failure),
        capture: None,
    }
}

impl AppModel {
    fn repo_undo(&self, repo_path: &str) -> Option<&RepoUndo> {
        self.undo.repos.get(repo_path)
    }

    fn active_repository_path(&self) -> Option<String> {
        self.active_repository().map(|repo| repo.path.clone())
    }

    /// "Undo commit 'Fix login'" (o, con `redo`, "Redo …") de lo próximo que
    /// se desharía en ese repositorio; `None` si no hay nada.
    pub fn undo_action_label(&self, repo_path: &str, redo: bool) -> Option<String> {
        self.repo_undo(repo_path)
            .and_then(|repo| repo.stack.peek(redo))
            .map(|entry| action_label(&entry.label, redo))
    }

    /// [`AppModel::undo_action_label`] del repositorio activo.
    pub fn active_undo_label(&self, redo: bool) -> Option<String> {
        self.active_repository()
            .and_then(|repo| self.undo_action_label(&repo.path, redo))
    }

    /// Las etiquetas de la pila (de la más reciente a la más vieja), sin
    /// "Undo" / "Redo": "commit 'Fix login'".
    pub fn undo_history(&self, repo_path: &str, redo: bool) -> Vec<String> {
        self.repo_undo(repo_path)
            .map(|repo| {
                repo.stack
                    .iter(redo)
                    .map(|entry| entry.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Hay un deshacer o rehacer en curso en ese repositorio.
    pub fn is_undo_running(&self, repo_path: &str) -> bool {
        self.undo.running.contains(repo_path)
    }

    /// Se puede deshacer (o rehacer) ahora en el repositorio activo: hay
    /// algo, no hay otro en curso ni una operación a medias.
    pub fn can_undo(&self, redo: bool) -> bool {
        let Some(repo) = self.active_repository() else {
            return false;
        };
        !self.is_undo_running(&repo.path)
            && !repo
                .operation_state
                .as_ref()
                .is_some_and(|state| state.in_progress)
            && self
                .repo_undo(&repo.path)
                .is_some_and(|undo| undo.stack.peek(redo).is_some())
    }

    /// Olvida la pila de un repositorio (y borra sus copias).
    pub fn clear_undo_history(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.undo.repos.get_mut(repo_path) else {
            return;
        };
        repo.pending = None;
        let dropped = repo.stack.clear();
        if !dropped.is_empty() {
            remove_dirs(dropped.into_iter().filter_map(|e| e.backup).collect(), cx);
            cx.notify();
        }
    }

    /// Carpeta nueva para la copia de un descarte:
    /// `<datos>/undo-backups/<sesión>/<n>`.
    pub(crate) fn next_discard_backup_dir(&mut self, cx: &mut Context<Self>) -> PathBuf {
        let root = match &self.undo.backup_root {
            Some(root) => root.clone(),
            None => {
                let base = self
                    .backend
                    .storage()
                    .path()
                    .and_then(Path::parent)
                    .map(|dir| dir.join("undo-backups"))
                    .unwrap_or_else(|| std::env::temp_dir().join("mergeon-undo-backups"));
                let millis = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |since| since.as_millis());
                let session = base.join(format!("{}-{millis}", std::process::id()));
                let (purge_base, purge_session) = (base.clone(), session.clone());
                cx.background_spawn(async move {
                    purge_stale_backups(&purge_base, &purge_session);
                })
                .detach();
                self.undo.backup_root = Some(session.clone());
                session
            }
        };
        self.undo.next_backup += 1;
        root.join(self.undo.next_backup.to_string())
    }

    /// La app se cierra: las copias de esta sesión ya no sirven.
    pub(crate) fn discard_undo_backups(&mut self) -> Option<PathBuf> {
        self.undo.backup_root.take()
    }

    /// Una mutación terminó: si se puede deshacer, a la pila.
    pub(crate) fn record_undo(
        &mut self,
        repo_path: &str,
        intent: &UndoIntent,
        capture: Option<&RefCapture>,
        summary: &OperationSummary,
        cx: &mut Context<Self>,
    ) {
        // Sin captura (el canal no la pide o la operación no llegó a la
        // cola) solo planifica lo que no la necesita: un bloque descartado o
        // un renombre.
        let empty = RefCapture::default();
        let capture = capture.unwrap_or(&empty);
        let repo = self.undo.repos.entry(repo_path.to_string()).or_default();
        let plan = plan_undo(intent, capture, summary, repo.pending.as_ref());
        match plan.pending {
            PendingChange::Keep => {}
            PendingChange::Set(pending) => repo.pending = Some(pending),
            PendingChange::Clear => repo.pending = None,
        }
        let mut dropped = Vec::new();
        // La copia de un descarte que no llegó a la pila no sirve.
        if let Some(DiscardBackupOutcome::Saved(backup)) = &capture.discard
            && plan
                .entry
                .as_ref()
                .is_none_or(|entry| entry.backup.as_ref() != Some(&backup.directory))
        {
            dropped.push(backup.directory.clone());
        }
        let recorded = plan.entry.is_some();
        if let Some(mut entry) = plan.entry {
            if matches!(intent, UndoIntent::Pull { .. }) {
                entry.confirm_published = false;
            }
            dropped.extend(
                repo.stack
                    .record(entry)
                    .into_iter()
                    .filter_map(|e| e.backup),
            );
        }
        remove_dirs(dropped, cx);
        if let Some(warning) = plan.warning {
            self.notify_warning(&warning, cx);
        }
        if recorded {
            cx.notify();
        }
    }

    /// Antes de descartar: si la copia pasaría del límite, pregunta si se
    /// descarta igualmente sin poder deshacer. Devuelve la carpeta de la
    /// copia (`None`: sin copia) o `Err` si el usuario dijo que no.
    pub(crate) fn prepare_discard_backup(
        &mut self,
        repo_path: &str,
        paths: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<(PathBuf, u64)>, ()>> {
        let directory = self.next_discard_backup_dir(cx);
        let repo = repo_path.to_string();
        let estimate = self.blocking(cx, move |backend| {
            backend
                .git()
                .estimate_discard_backup_size(&repo, paths.as_deref())
        });
        flow(cx, async move |this, cx| {
            let bytes = estimate.await;
            if bytes <= DISCARD_BACKUP_LIMIT {
                return Ok(Some((directory, DISCARD_BACKUP_LIMIT)));
            }
            let confirmed = this
                .update(cx, |this, cx| {
                    this.confirm(
                        ConfirmOptions {
                            title: Some("Discard without undo?".to_string()),
                            description: Some(format!(
                                "These changes ({}) are larger than the {} Mergeon keeps to undo a discard. If you discard them, they can't be recovered.",
                                format_bytes(bytes),
                                format_bytes(DISCARD_BACKUP_LIMIT)
                            )),
                            confirm_text: Some("Discard anyway".to_string()),
                            cancel_text: Some("Cancel".to_string()),
                            variant: Some("destructive".to_string()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            if confirmed { Ok(None) } else { Err(()) }
        })
    }

    /// `prepare_discard_backup` y, si sigue, `discard(backup)`.
    pub(crate) fn discard_with_backup(
        &mut self,
        repo_path: &str,
        paths: Option<Vec<String>>,
        operation: &'static str,
        cx: &mut Context<Self>,
        discard: impl FnOnce(&mut Self, Option<(PathBuf, u64)>, &mut Context<Self>) -> Task<GitMutation>
        + 'static,
    ) -> Task<GitMutation> {
        let prepared = self.prepare_discard_backup(repo_path, paths, cx);
        flow(cx, async move |this, cx| match prepared.await {
            Ok(backup) => this.update(cx, |this, cx| discard(this, backup, cx)).await,
            Err(()) => cancelled_mutation(operation, "Discard cancelled."),
        })
    }

    /// Tras detectar un cambio externo de HEAD (watcher o volver a una pestaña
    /// oculta), si el tope de la pila ya no coincide con el HEAD/rama actual,
    /// vacía la pila para que el botón ↶ se apague al instante en vez de ofrecer
    /// una acción que fallará al pulsarla.
    pub(crate) fn reconcile_undo_after_external_head_change(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) {
        if self.is_undo_running(repo_path) {
            return;
        }
        let Some(repository) = self
            .repositories
            .repositories
            .iter()
            .find(|repository| repository.path == repo_path)
        else {
            return;
        };
        if repository.mutation_queue.active.is_some() {
            return;
        }
        let Some(status) = repository.status.as_ref() else {
            return;
        };
        let current_head = status.head_short_hash.as_deref();
        let current_branch = status
            .current
            .as_deref()
            .filter(|name| !name.is_empty() && *name != "HEAD");

        let step_stale = |step: &UndoStep| -> bool {
            match step {
                UndoStep::MoveHead { branch, from, .. } => {
                    let head_moved = current_head
                        .is_some_and(|short| !short.is_empty() && !from.starts_with(short));
                    let branch_moved = match branch.as_deref() {
                        Some(expected) => current_branch != Some(expected),
                        None => current_branch.is_some(),
                    };
                    head_moved || branch_moved
                }
                UndoStep::Checkout { from, .. } => match from {
                    HeadTarget::Branch(expected) => current_branch != Some(expected.as_str()),
                    HeadTarget::Detached(sha) => {
                        current_branch.is_some()
                            || current_head
                                .is_some_and(|short| !short.is_empty() && !sha.starts_with(short))
                    }
                },
                _ => false,
            }
        };

        let stale = self.repo_undo(repo_path).is_some_and(|repo| {
            let undo_stale = repo
                .stack
                .peek(false)
                .is_some_and(|entry| entry.undo.iter().any(&step_stale));
            let redo_stale = repo
                .stack
                .peek(true)
                .is_some_and(|entry| entry.redo.iter().any(&step_stale));
            undo_stale || redo_stale
        });
        if stale {
            let mut dropped = Vec::new();
            if let Some(repo) = self.undo.repos.get_mut(repo_path) {
                dropped = repo.stack.clear();
                repo.pending = None;
            }
            remove_dirs(dropped.into_iter().filter_map(|e| e.backup).collect(), cx);
            cx.notify();
        }
    }

    /// Deshace lo último del repositorio activo. Devuelve si se deshizo.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        match self.active_repository_path() {
            Some(path) => self.run_undo(&path, false, cx),
            None => Task::ready(false),
        }
    }

    /// Rehace lo último deshecho en el repositorio activo.
    pub fn redo(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        match self.active_repository_path() {
            Some(path) => self.run_undo(&path, true, cx),
            None => Task::ready(false),
        }
    }

    /// Deshace (o rehace) lo último de `repo_path`: confirma si quitaría
    /// commits publicados, corre la mutación `undo` / `redo` por la cola,
    /// avisa del resultado y relee el repositorio. Si la entrada ya no
    /// aplica (el repositorio cambió por fuera), vacía la pila y lo dice.
    pub fn run_undo(&mut self, repo_path: &str, redo: bool, cx: &mut Context<Self>) -> Task<bool> {
        let verb = if redo { "redo" } else { "undo" };
        let Some(repository) = self
            .repositories
            .repositories
            .iter()
            .find(|repository| repository.path == repo_path)
        else {
            return Task::ready(false);
        };
        let repo_id = repository.id.clone();
        if self.is_undo_running(repo_path) {
            return Task::ready(false);
        }
        if let Some(state) = repository
            .operation_state
            .as_ref()
            .filter(|state| state.in_progress)
        {
            let what = state.label.trim().to_lowercase();
            let message = if what.is_empty() {
                format!("Finish or abort the operation in progress before you {verb}.")
            } else if what.ends_with("in progress") {
                format!("Finish or abort the {what} before you {verb}.")
            } else {
                format!("Finish or abort the {what} in progress before you {verb}.")
            };
            self.notify_info(&message, cx);
            return Task::ready(false);
        }
        let Some(entry) = self
            .repo_undo(repo_path)
            .and_then(|repo| repo.stack.peek(redo))
            .cloned()
        else {
            let message = if redo {
                "Nothing to redo."
            } else {
                "Nothing to undo."
            };
            self.notify_info(message, cx);
            return Task::ready(false);
        };
        let steps = if redo {
            entry.redo.clone()
        } else {
            entry.undo.clone()
        };
        let action = action_label(&entry.label, redo);
        let repo_path = repo_path.to_string();
        self.undo.running.insert(repo_path.clone());
        cx.notify();

        // ¿Quitaría commits que ya están en el upstream?
        let moves = if entry.confirm_published {
            head_moves(&steps)
        } else {
            Vec::new()
        };
        let published = {
            let repo = repo_path.clone();
            self.blocking(cx, move |backend| {
                moves.iter().find_map(|(branch, from, to)| {
                    backend
                        .git()
                        .undo_published_upstream(&repo, branch, from, to)
                })
            })
        };

        flow(cx, async move |this, cx| {
            let finish = |this: &mut AppModel, path: &str, cx: &mut Context<AppModel>| {
                this.undo.running.remove(path);
                cx.notify();
            };
            if let Some(upstream) = published.await {
                let confirmed = this
                    .update(cx, |this, cx| {
                        this.confirm(
                            ConfirmOptions {
                                title: Some(format!("{action}?")),
                                description: Some(format!(
                                    "This rewrites commits that are already on {upstream}. To publish the result you'll need to force push; Mergeon won't push anything for you."
                                )),
                                confirm_text: Some(
                                    if redo { "Redo anyway" } else { "Undo anyway" }.to_string(),
                                ),
                                cancel_text: Some("Cancel".to_string()),
                                variant: Some("destructive".to_string()),
                                ..Default::default()
                            },
                            cx,
                        )
                    })
                    .await;
                if !confirmed {
                    this.update(cx, |this, cx| finish(this, &repo_path, cx));
                    return false;
                }
            }

            // La entrada sale de la pila mientras corre (si cambió entre
            // medias, no se toca nada).
            let taken = this.update(cx, |this, _| {
                let repo = this.undo.repos.get_mut(&repo_path)?;
                (repo.stack.peek(redo) == Some(&entry))
                    .then(|| repo.stack.take(redo))
                    .flatten()
            });
            let Some(entry) = taken else {
                this.update(cx, |this, cx| finish(this, &repo_path, cx));
                return false;
            };

            let result = this
                .update(cx, |this, cx| {
                    this.undo_mutation(&repo_path, &entry.label, steps, redo, cx)
                })
                .await;
            let summary = crate::convert::StructuredOutcome::to_operation_result(&result);
            let done = summary.success;
            this.update(cx, |this, cx| {
                finish(this, &repo_path, cx);
                let restore_message = (!redo)
                    .then(|| entry.restore_commit_message.clone())
                    .flatten();
                let undo_note = (!redo).then(|| entry.undo_note.clone()).flatten();
                let message = summary.message.clone().unwrap_or_default();
                let message = this.message_for_repository(&repo_path, &message);
                let mut dropped = Vec::new();
                if let Some(repo) = this.undo.repos.get_mut(&repo_path) {
                    match summary.code.as_deref() {
                        _ if done => dropped = repo.stack.finish(redo, entry),
                        Some("UNDO_BLOCKED") => repo.stack.put_back(redo, entry),
                        Some("CANCELLED" | "TIMEOUT") => repo.stack.put_back(redo, entry),
                        _ => {
                            // Ya no aplica (o quedó a medias): la pila entera
                            // describe un repositorio que ya no existe.
                            dropped = repo.stack.clear();
                            dropped.push(entry);
                            repo.pending = None;
                        }
                    }
                }
                remove_dirs(dropped.into_iter().filter_map(|e| e.backup).collect(), cx);
                match summary.code.as_deref() {
                    _ if done => {
                        let shown = match undo_note {
                            Some(note) => format!("{message} {note}"),
                            None => message,
                        };
                        this.notify_success(&shown, cx);
                        if let Some(commit_message) = restore_message
                            && this.commit_draft(&repo_path).trim().is_empty()
                        {
                            this.set_commit_draft(&repo_path, &commit_message, cx);
                            cx.emit(crate::AppEvent::CommitDraftRestored {
                                repo_path: repo_path.clone(),
                                message: commit_message,
                            });
                        }
                    }
                    Some("UNDO_STALE") => {
                        this.notify_warning(
                            &format!(
                                "{message} The repository changed outside Mergeon, so its undo history was cleared."
                            ),
                            cx,
                        );
                    }
                    Some("UNDO_BLOCKED") => {
                        this.notify_warning(&message, cx);
                    }
                    Some("CANCELLED") => {}
                    _ => {
                        this.notify_error(
                            &format!("{message} The undo history of this repository was cleared."),
                            cx,
                        );
                    }
                }
                this.apply_repository_operation_result(repo_id.as_str(), &result, cx);
                this.sync_repository_read_model(
                    repo_id.as_str(),
                    SyncRequest::new()
                        .status(true)
                        .changes_count(true)
                        .branches(true)
                        .tags(true)
                        .stashes(true)
                        .operation(verb)
                        .result(result.clone()),
                    cx,
                )
            })
            .await;
            done
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_backend::undo::HeadTarget;

    #[test]
    fn a_new_operation_clears_redo_and_the_oldest_falls_off_the_top() {
        let mut stack = UndoStack::new(3);
        assert!(stack.record(1).is_empty());
        stack.record(2);
        stack.record(3);
        assert_eq!(stack.record(4), [1]);
        assert_eq!(stack.iter(false).copied().collect::<Vec<_>>(), [4, 3, 2]);

        let top = stack.take(false).unwrap();
        assert_eq!(top, 4);
        assert!(stack.finish(false, top).is_empty());
        assert_eq!(stack.peek(true), Some(&4));
        assert_eq!(stack.peek(false), Some(&3));

        // Algo nuevo: lo que había para rehacer se pierde.
        assert_eq!(stack.record(5), [4]);
        assert_eq!(stack.len(true), 0);
    }

    #[test]
    fn a_failed_undo_goes_back_where_it_was_and_clear_empties_both() {
        let mut stack = UndoStack::new(UNDO_LIMIT);
        stack.record("a");
        stack.record("b");
        let taken = stack.take(false).unwrap();
        stack.put_back(false, taken);
        assert_eq!(stack.peek(false), Some(&"b"));

        let taken = stack.take(false).unwrap();
        stack.finish(false, taken);
        let redo = stack.take(true).unwrap();
        stack.put_back(true, redo);
        assert_eq!(stack.peek(true), Some(&"b"));
        let redo = stack.take(true).unwrap();
        stack.finish(true, redo);
        assert_eq!(stack.peek(false), Some(&"b"));

        assert_eq!(stack.clear(), ["a", "b"]);
        assert!(stack.is_empty());
    }

    fn snapshot(head: &str, branch: Option<&str>) -> RefSnapshot {
        RefSnapshot {
            head: Some(head.to_string()),
            head_ref: branch.map(|branch| format!("refs/heads/{branch}")),
            ..Default::default()
        }
    }

    fn completed() -> OperationSummary {
        OperationSummary {
            success: true,
            outcome: Some("completed".into()),
            code: Some("OK".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_commit_plans_a_soft_reset_back_and_forth() {
        let capture = RefCapture {
            before: snapshot("aaa", Some("main")),
            after: snapshot("bbb", Some("main")),
            discard: None,
        };
        let intent = UndoIntent::Commit {
            message: "Fix login\n\ndetails".into(),
            amend: false,
            fixup: false,
        };
        let plan = plan_undo(&intent, &capture, &completed(), None);
        let entry = plan.entry.expect("entrada");
        assert_eq!(entry.label, "commit 'Fix login'");
        assert_eq!(
            entry.undo,
            [UndoStep::MoveHead {
                branch: Some("main".into()),
                from: "bbb".into(),
                to: "aaa".into(),
                mode: ResetMode::Soft,
            }]
        );
        assert_eq!(
            entry.redo,
            [UndoStep::MoveHead {
                branch: Some("main".into()),
                from: "aaa".into(),
                to: "bbb".into(),
                mode: ResetMode::Soft,
            }]
        );

        // Un commit fallido o que no movió HEAD no deja nada.
        let failed = OperationSummary {
            success: false,
            ..completed()
        };
        assert_eq!(plan_undo(&intent, &capture, &failed, None).entry, None);
        let still = RefCapture {
            after: snapshot("aaa", Some("main")),
            ..capture
        };
        assert_eq!(plan_undo(&intent, &still, &completed(), None).entry, None);
    }

    #[test]
    fn a_conflicted_sequence_waits_for_the_commit_that_concludes_it() {
        let conflicted = OperationSummary {
            success: false,
            outcome: Some("conflicted".into()),
            code: Some("CONFLICT".into()),
            ..Default::default()
        };
        let merge = UndoIntent::Sequence {
            label: "merge of feature".into(),
        };
        let at_start = RefCapture {
            before: snapshot("aaa", Some("main")),
            after: snapshot("aaa", Some("main")),
            discard: None,
        };
        let plan = plan_undo(&merge, &at_start, &conflicted, None);
        let PendingChange::Set(pending) = plan.pending else {
            panic!("se esperaba una operación pendiente");
        };
        assert_eq!(plan.entry, None);

        let mut before = snapshot("aaa", Some("main"));
        before.refs.insert("MERGE_HEAD".into(), "fff".into());
        let concluded = RefCapture {
            before,
            after: snapshot("mmm", Some("main")),
            discard: None,
        };
        let commit = UndoIntent::Commit {
            message: "Merge branch 'feature'".into(),
            amend: false,
            fixup: false,
        };
        let plan = plan_undo(&commit, &concluded, &completed(), Some(&pending));
        assert_eq!(plan.pending, PendingChange::Clear);
        let entry = plan.entry.expect("entrada");
        assert_eq!(entry.label, "merge of feature");
        assert!(matches!(
            &entry.undo[..],
            [UndoStep::MoveHead { to, mode: ResetMode::Keep, .. }] if to == "aaa"
        ));
    }

    #[test]
    fn a_sequence_on_another_branch_undoes_the_branch_and_returns_to_head() {
        // Arrastrar `feature` sobre `topic` estando en `main`: rebase de
        // `topic` (que estaba en ttt) sobre `feature`.
        let mut before = snapshot("mmm", Some("main"));
        before.refs.insert("refs/heads/topic".into(), "ttt".into());
        let capture = RefCapture {
            before,
            after: snapshot("rrr", Some("topic")),
            discard: None,
        };
        let rebase = UndoIntent::Sequence {
            label: "rebase onto feature".into(),
        };
        let plan = plan_undo(&rebase, &capture, &completed(), None);
        let entry = plan.entry.expect("entrada");
        assert_eq!(
            entry.undo,
            [
                UndoStep::MoveHead {
                    branch: Some("topic".into()),
                    from: "rrr".into(),
                    to: "ttt".into(),
                    mode: ResetMode::Keep,
                },
                UndoStep::Checkout {
                    from: HeadTarget::Branch("topic".into()),
                    to: HeadTarget::Branch("main".into()),
                },
            ]
        );
        assert_eq!(
            entry.redo,
            [
                UndoStep::Checkout {
                    from: HeadTarget::Branch("main".into()),
                    to: HeadTarget::Branch("topic".into()),
                },
                UndoStep::MoveHead {
                    branch: Some("topic".into()),
                    from: "ttt".into(),
                    to: "rrr".into(),
                    mode: ResetMode::Keep,
                },
            ]
        );

        // Con conflictos (rebase a medias: HEAD desanclado), lo pendiente es
        // la rama destino y dónde estaba.
        let conflicted = OperationSummary {
            success: false,
            outcome: Some("conflicted".into()),
            code: Some("CONFLICT".into()),
            ..Default::default()
        };
        let mut before = snapshot("mmm", Some("main"));
        before.refs.insert("refs/heads/topic".into(), "ttt".into());
        let stopped = RefCapture {
            before,
            after: snapshot("xxx", None),
            discard: None,
        };
        let plan = plan_undo(&rebase, &stopped, &conflicted, None);
        assert_eq!(
            plan.pending,
            PendingChange::Set(PendingSequence {
                label: "rebase onto feature".into(),
                branch: Some("topic".into()),
                before: "ttt".into(),
            })
        );
    }

    #[test]
    fn deleting_a_branch_remembers_its_hash_and_upstream() {
        let mut before = snapshot("aaa", Some("main"));
        before.refs.insert("refs/heads/foo".into(), "f00".into());
        before
            .configs
            .insert("branch.foo.remote".into(), "origin".into());
        before
            .configs
            .insert("branch.foo.merge".into(), "refs/heads/foo".into());
        let capture = RefCapture {
            before,
            after: snapshot("aaa", Some("main")),
            discard: None,
        };
        let plan = plan_undo(
            &UndoIntent::DeleteBranch { name: "foo".into() },
            &capture,
            &completed(),
            None,
        );
        let entry = plan.entry.expect("entrada");
        assert_eq!(
            action_label(&entry.label, false),
            "Undo delete branch 'foo'"
        );
        assert_eq!(
            entry.undo,
            [UndoStep::CreateBranch {
                name: "foo".into(),
                sha: "f00".into(),
                upstream: Some(("origin".into(), "refs/heads/foo".into())),
            }]
        );
    }

    #[test]
    fn a_checkout_plans_both_directions() {
        let capture = RefCapture {
            before: snapshot("aaa", Some("main")),
            after: snapshot("bbb", None),
            discard: None,
        };
        let plan = plan_undo(
            &UndoIntent::Checkout {
                label: "v1.0".into(),
            },
            &capture,
            &completed(),
            None,
        );
        let entry = plan.entry.expect("entrada");
        assert_eq!(entry.label, "checkout of 'v1.0'");
        assert_eq!(
            entry.undo,
            [UndoStep::Checkout {
                from: HeadTarget::Detached("bbb".into()),
                to: HeadTarget::Branch("main".into()),
            }]
        );
    }

    #[test]
    fn a_discard_without_a_copy_warns() {
        let capture = RefCapture {
            before: snapshot("aaa", Some("main")),
            after: snapshot("aaa", Some("main")),
            discard: Some(DiscardBackupOutcome::TooLarge(80 * 1024 * 1024)),
        };
        let plan = plan_undo(
            &UndoIntent::Discard {
                label: "discard of all changes".into(),
            },
            &capture,
            &completed(),
            None,
        );
        assert_eq!(plan.entry, None);
        assert!(plan.warning.unwrap().contains("80 MB"));
    }

    #[test]
    fn labels_are_short_and_readable() {
        assert_eq!(commit_subject("\n  Fix login  \nbody"), "Fix login");
        let long = "x".repeat(80);
        assert_eq!(commit_subject(&long).chars().count(), 50);
        assert!(commit_subject(&long).ends_with('…'));
        assert_eq!(format_bytes(50 * 1024 * 1024), "50 MB");
        assert_eq!(format_bytes(1536 * 1024), "1.5 MB");
        assert_eq!(format_bytes(10), "1 KB");
        assert_eq!(
            action_label("merge of feature/x", true),
            "Redo merge of feature/x"
        );
    }
}
