//! Relectura dirigida del estado de un repositorio
//! (`src/composables/useRepositorySync.js`).
//!
//! Tras una mutación no se relee todo: quien la hizo dice qué secciones
//! pudieron cambiar ([`SyncRequest`]) y [`AppModel::sync_repository_read_model`]
//! lee solo esas, las aplica al repositorio y, si se indica la operación,
//! registra la mutación.
//!
//! Dos garantías del original se conservan:
//!
//! - **Lecturas compartidas**: dos sincronizaciones que piden la misma sección
//!   del mismo repositorio a la vez comparten una sola lectura.
//! - **Generaciones**: una lectura que empezó antes de una mutación no pisa el
//!   estado que esa mutación dejó. Cada mutación avanza la generación del
//!   repositorio y las respuestas de generaciones anteriores se descartan.

use std::collections::HashMap;

use gpui_kit::{Context, Task};
use mergeon_backend::{
    BranchesOverview, QueryError, QueryResult, Remote, StashEntry, StructuredStatus, Submodule,
};
use mergeon_model::format::now_iso_string;
use mergeon_model::notify::{NotifyError, NotifyOptions, NotifyType};
use mergeon_model::operation_types::OperationResult;
use mergeon_model::repositories::{RepoKey, SidebarSection};
use mergeon_model::repository_read_instrumentation::measure_repository_read;
use mergeon_model::repository_read_model::{RemoteItem, build_branch_items, build_remote_items};

use crate::convert::{branch_data, remote_data};
use crate::repositories::tracked;
use crate::tasks::{InFlightMap, flow, settled};
use crate::{AppEvent, AppModel};

/// Estado de módulo de `useRepositorySync`: generaciones de lectura y lecturas
/// en vuelo por sección.
#[derive(Debug, Default)]
pub(crate) struct SyncState {
    pub read_generations: HashMap<String, u64>,
    pub in_flight_section_reads: InFlightMap,
    /// Lecturas arrancadas por sección (`<repo_id>:<sección>`), para comprobar
    /// que las peticiones simultáneas se comparten.
    pub started_reads: HashMap<String, usize>,
}

fn in_flight_section_reads(model: &mut AppModel) -> &mut InFlightMap {
    &mut model.sync.in_flight_section_reads
}

/// Qué releer. Los campos de sección equivalen a las claves del objeto
/// `requested` del original.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncRequest {
    pub status: bool,
    pub branches: bool,
    pub remotes: bool,
    pub tags: bool,
    pub stashes: bool,
    pub submodules: bool,
    pub changes_count: bool,
    pub starred_branches: bool,
    /// Resultado de la mutación que motiva la relectura. Si trae `status` se
    /// aplica tal cual en vez de leerlo otra vez.
    pub result: Option<OperationResult>,
    /// Operación que se registra como última mutación del repositorio
    /// (`lastMutationOperation`) al terminar.
    pub operation: Option<String>,
}

impl SyncRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(mut self, enabled: bool) -> Self {
        self.status = enabled;
        self
    }

    pub fn branches(mut self, enabled: bool) -> Self {
        self.branches = enabled;
        self
    }

    pub fn remotes(mut self, enabled: bool) -> Self {
        self.remotes = enabled;
        self
    }

    pub fn tags(mut self, enabled: bool) -> Self {
        self.tags = enabled;
        self
    }

    pub fn stashes(mut self, enabled: bool) -> Self {
        self.stashes = enabled;
        self
    }

    pub fn submodules(mut self, enabled: bool) -> Self {
        self.submodules = enabled;
        self
    }

    pub fn changes_count(mut self, enabled: bool) -> Self {
        self.changes_count = enabled;
        self
    }

    pub fn starred_branches(mut self, enabled: bool) -> Self {
        self.starred_branches = enabled;
        self
    }

    pub fn result(mut self, result: impl crate::StructuredOutcome) -> Self {
        self.result = Some(result.to_operation_result());
        self
    }

    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    /// `operation: result?.operation || fallback`.
    pub fn operation_or(mut self, operation: Option<&str>, fallback: &str) -> Self {
        self.operation = Some(
            operation
                .filter(|operation| !operation.is_empty())
                .unwrap_or(fallback)
                .to_string(),
        );
        self
    }
}

/// Lo que leyó una sincronización. Cada campo es `None` si esa sección no se
/// pidió o su lectura falló.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncOutcome {
    pub status: Option<StructuredStatus>,
    pub branch_data: Option<BranchesOverview>,
    pub remotes: Option<Vec<Remote>>,
    pub tags: Option<Vec<String>>,
    pub stashes: Option<Vec<StashEntry>>,
    pub submodules: Option<Vec<Submodule>>,
    pub starred_branches: Option<Vec<String>>,
}

/// `mergeRemoteBranchLists`: los remotos recién leídos con las ramas que ya se
/// conocían de cada uno.
fn merge_remote_branch_lists(remotes_data: &[Remote], existing: &[RemoteItem]) -> Vec<RemoteItem> {
    remotes_data
        .iter()
        .map(|remote| {
            let data = remote_data(remote);
            RemoteItem {
                branches: existing
                    .iter()
                    .find(|item| item.name == remote.name)
                    .map(|item| item.branches.clone())
                    .unwrap_or_default(),
                name: data.name,
                refs: data.refs,
            }
        })
        .collect()
}

/// La lectura que una relectura arranca nada más pedirse (ver
/// [`AppModel::sync_repository_read_model`]).
enum FirstRead {
    None,
    // En caja: un `Task` ya resuelto lleva dentro el estado completo.
    Status(Box<Task<QueryResult<StructuredStatus>>>),
    Branches(Task<QueryResult<BranchesOverview>>),
    Tags(Task<QueryResult<Vec<String>>>),
    Stashes(Task<QueryResult<Vec<StashEntry>>>),
    Submodules(Task<QueryResult<Vec<Submodule>>>),
    StarredBranches(Task<QueryResult<Vec<String>>>),
}

impl FirstRead {
    fn take(&mut self) -> FirstRead {
        std::mem::replace(self, FirstRead::None)
    }
}

fn cancelled() -> QueryError {
    QueryError::read("Repository read", "The read was cancelled")
}

impl AppModel {
    fn get_read_generation(&self, repo_id: &str) -> u64 {
        self.sync
            .read_generations
            .get(repo_id)
            .copied()
            .unwrap_or(0)
    }

    fn invalidate_read_generation(&mut self, repo_id: &str) {
        let next = self.get_read_generation(repo_id) + 1;
        self.sync.read_generations.insert(repo_id.to_string(), next);
        self.sync
            .in_flight_section_reads
            .remove_prefix(&format!("{repo_id}:"));
    }

    /// `canApplyRead`: la lectura sigue siendo de la generación actual y el
    /// repositorio sigue abierto.
    fn can_apply_read(&self, repo_id: &str, read_generation: u64) -> bool {
        self.get_read_generation(repo_id) == read_generation
            && self.repositories.repository(repo_id).is_some()
    }

    /// Lecturas arrancadas de una sección de un repositorio. Dos
    /// sincronizaciones simultáneas de la misma sección cuentan una.
    #[doc(hidden)]
    pub fn started_section_reads(&self, repo_id: &str, section: &str) -> usize {
        self.sync
            .started_reads
            .get(&format!("{repo_id}:{section}"))
            .copied()
            .unwrap_or(0)
    }

    /// `readSection`: una lectura por repositorio y sección a la vez.
    fn read_section<T>(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        section: &'static str,
        trace_name: &'static str,
        cx: &mut Context<Self>,
        reader: impl FnOnce(&mergeon_backend::Backend, &str) -> QueryResult<T> + Send + 'static,
    ) -> Task<QueryResult<T>>
    where
        T: Clone + Send + 'static,
    {
        let key = format!("{repo_id}:{section}");
        let repo_path = repo_path.to_string();
        let counter_key = key.clone();
        let shared = self.run_shared(in_flight_section_reads, key, false, cx, move |this, cx| {
            *this.sync.started_reads.entry(counter_key).or_default() += 1;
            this.blocking(cx, move |backend| {
                measure_repository_read(trace_name, &repo_path, || reader(backend, &repo_path))
            })
        });

        cx.foreground_executor()
            .spawn(async move { settled(shared).await.unwrap_or_else(|| Err(cancelled())) })
    }

    /// Avisa de una lectura fallida como lo hacían, una tras otra, `useGit`
    /// (`Error loading ...`) y `useRepositorySync` (`Could not ...`): las dos
    /// capas avisan del mismo error, así que solo se ve el primer aviso.
    fn notify_section_failure(
        &mut self,
        repo_path: &str,
        log_context: &str,
        titles: &[&str],
        error: &QueryError,
        cx: &mut Context<Self>,
    ) {
        log::error!("{log_context} {repo_path}: {error}");
        let token = self.notifier.error_token();
        for title in titles {
            self.notify(
                NotifyType::Error,
                title,
                NotifyOptions {
                    error: Some(NotifyError::with_token(error.message(), token)),
                    ..Default::default()
                },
                cx,
            );
        }
    }

    /// `recordMutation`: invalida las lecturas en vuelo del repositorio y anota
    /// la mutación (`lastMutationAt`, `lastMutationOperation`). Las vistas que
    /// dependen de ella se enteran por [`AppEvent::MutationRecorded`]; no se
    /// avisa a `observe`, porque lo que se ve no cambia por anotarla (lo que
    /// trajo la relectura ya avisó si cambió algo). Así una relectura del
    /// watcher que encuentra lo mismo no repinta la ventana.
    pub fn record_mutation<'a>(
        &mut self,
        repo: impl Into<RepoKey<'a>>,
        operation: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self
            .repositories
            .repository(repo)
            .map(|repository| repository.id.clone())
        else {
            return;
        };
        self.invalidate_read_generation(&repo_id);

        if let Some(repository) = self.repositories.repository_mut(repo_id.as_str()) {
            repository.last_mutation_at = Some(now_iso_string());
            repository.last_mutation_operation = operation.map(str::to_string);
        }
        cx.emit(AppEvent::MutationRecorded {
            repo_id,
            operation: operation.map(str::to_string),
        });
    }

    fn start_status_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<StructuredStatus>> {
        self.read_section(
            repo_id,
            repo_path,
            "status",
            "getStatus",
            cx,
            |backend, path| backend.git().get_status(path),
        )
    }

    fn start_branches_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<BranchesOverview>> {
        self.read_section(
            repo_id,
            repo_path,
            "branches",
            "getBranches",
            cx,
            |backend, path| backend.git().get_branches(path),
        )
    }

    fn start_remotes_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Remote>>> {
        self.read_section(
            repo_id,
            repo_path,
            "remotes",
            "getRemotes",
            cx,
            |backend, path| backend.git().get_remotes(path),
        )
    }

    fn start_tags_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<String>>> {
        self.read_section(
            repo_id,
            repo_path,
            "tags",
            "getTags",
            cx,
            |backend, path| backend.git().get_tags(path),
        )
    }

    fn start_stashes_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<StashEntry>>> {
        self.read_section(
            repo_id,
            repo_path,
            "stashes",
            "getStashes",
            cx,
            |backend, path| backend.git().get_stashes(path),
        )
    }

    fn start_submodules_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Submodule>>> {
        self.read_section(
            repo_id,
            repo_path,
            "submodules",
            "getSubmodules",
            cx,
            |backend, path| backend.git().get_submodules(path),
        )
    }

    fn start_starred_branches_read(
        &mut self,
        repo_id: &str,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<String>>> {
        self.read_section(
            repo_id,
            repo_path,
            "starredBranches",
            "getStarredBranches",
            cx,
            |backend, path| Ok(backend.storage().get_starred_branches(path)),
        )
    }

    /// `syncRepositoryReadModel`: relee las secciones pedidas y las aplica al
    /// repositorio. Devuelve `None` si el repositorio no está abierto o si una
    /// mutación posterior dejó obsoleta esta relectura.
    pub fn sync_repository_read_model<'a>(
        &mut self,
        repo: impl Into<RepoKey<'a>>,
        request: SyncRequest,
        cx: &mut Context<Self>,
    ) -> Task<Option<SyncOutcome>> {
        let Some(repository) = self.repositories.repository(repo) else {
            return Task::ready(None);
        };
        // Como en el original, lo que se lee del repositorio durante la
        // relectura es lo que había al empezarla.
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();
        let known_remotes = repository.remotes.clone();

        let sections = request;
        let result = sections.result.clone();
        if result.is_some() || sections.operation.is_some() {
            self.invalidate_read_generation(&repo_id);
        }
        let read_generation = self.get_read_generation(&repo_id);

        // El estado que trae el resultado se aplica en el acto, antes de
        // cualquier espera: es más nuevo que cualquier lectura en vuelo.
        let result_status = result.as_ref().and_then(|result| result.status.clone());
        let mut applied_result_status = false;
        if let Some(status) = &result_status
            && self.can_apply_read(&repo_id, read_generation)
        {
            self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                tracked::set_status(repo, Some(status.clone()))
            });
            applied_result_status = true;
        }

        /*
         * La primera lectura arranca (y queda registrada como lectura en
         * vuelo) dentro de esta misma llamada, igual que en el original, donde
         * la función corría hasta su primer `await`. Así dos relecturas pedidas
         * a la vez comparten la lectura, y una mutación que llegue enseguida
         * encuentra la lectura ya empezada y la deja obsoleta.
         */
        let wants_status = !applied_result_status && (sections.status || sections.changes_count);
        let wants_branches = sections.branches || sections.remotes;
        let mut first_read = FirstRead::None;
        if wants_status {
            first_read =
                FirstRead::Status(Box::new(self.start_status_read(&repo_id, &repo_path, cx)));
        } else if self.can_apply_read(&repo_id, read_generation) {
            if wants_branches {
                self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                    tracked::set_sidebar_loading(repo, SidebarSection::Branches, true)
                });
                first_read =
                    FirstRead::Branches(self.start_branches_read(&repo_id, &repo_path, cx));
            } else if sections.tags {
                self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                    tracked::set_sidebar_loading(repo, SidebarSection::Tags, true)
                });
                first_read = FirstRead::Tags(self.start_tags_read(&repo_id, &repo_path, cx));
            } else if sections.stashes {
                self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                    tracked::set_sidebar_loading(repo, SidebarSection::Stashes, true)
                });
                first_read = FirstRead::Stashes(self.start_stashes_read(&repo_id, &repo_path, cx));
            } else if sections.submodules {
                self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                    tracked::set_sidebar_loading(repo, SidebarSection::Submodules, true)
                });
                first_read =
                    FirstRead::Submodules(self.start_submodules_read(&repo_id, &repo_path, cx));
            } else if sections.starred_branches {
                self.update_repository_soon(repo_id.as_str(), cx, |repo| {
                    tracked::set_sidebar_loading(repo, SidebarSection::StarredBranches, true)
                });
                first_read = FirstRead::StarredBranches(
                    self.start_starred_branches_read(&repo_id, &repo_path, cx),
                );
            }
        }

        flow(cx, async move |this, cx| {
            let can_apply = |this: &AppModel| this.can_apply_read(&repo_id, read_generation);
            let mut outcome = SyncOutcome::default();

            if applied_result_status {
                outcome.status = result_status.clone();
            } else if sections.status || sections.changes_count {
                let read = match first_read.take() {
                    FirstRead::Status(read) => *read,
                    _ => this.update(cx, |this, cx| {
                        this.start_status_read(&repo_id, &repo_path, cx)
                    }),
                };
                match read.await {
                    Ok(status) => {
                        this.update(cx, |this, cx| {
                            if can_apply(this) {
                                this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                    tracked::set_status(repo, Some(status.clone()))
                                });
                            }
                        });
                        outcome.status = Some(status);
                    }
                    Err(error) => this.update(cx, |this, cx| {
                        this.notify_section_failure(
                            &repo_path,
                            "read status failed",
                            &[
                                "Error loading status",
                                "Could not read the repository status",
                            ],
                            &error,
                            cx,
                        );
                    }),
                }
            }

            let started = matches!(first_read, FirstRead::Branches(_));
            if wants_branches && (started || this.read_with(cx, |this, _| can_apply(this))) {
                let read = match first_read.take() {
                    FirstRead::Branches(read) => read,
                    _ => this.update(cx, |this, cx| {
                        this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                            tracked::set_sidebar_loading(repo, SidebarSection::Branches, true)
                        });
                        this.start_branches_read(&repo_id, &repo_path, cx)
                    }),
                };
                let branches = read.await;
                let stale = this.update(cx, |this, cx| {
                    let stale = !can_apply(this);
                    match &branches {
                        Ok(overview) if !stale => {
                            let data = branch_data(overview);
                            this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                tracked::set_branches_data(
                                    repo,
                                    build_branch_items(&data),
                                    Some(&data.current),
                                )
                            });
                            this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                tracked::update_branch(repo, Some(&data.current))
                            });
                        }
                        Ok(_) => {}
                        Err(error) => this.notify_section_failure(
                            &repo_path,
                            "read branches failed",
                            &["Error loading branches", "Could not load branches"],
                            error,
                            cx,
                        ),
                    }
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Branches, false)
                    });
                    stale && branches.is_ok()
                });
                if stale {
                    return None;
                }
                outcome.branch_data = branches.ok();
            }

            if sections.remotes && this.read_with(cx, |this, _| can_apply(this)) {
                let read = this.update(cx, |this, cx| {
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Remotes, true)
                    });
                    this.start_remotes_read(&repo_id, &repo_path, cx)
                });

                let mut stale = false;
                match read.await {
                    Ok(remotes) if this.read_with(cx, |this, _| can_apply(this)) => {
                        let mut remote_branch_data = outcome.branch_data.clone();
                        if remote_branch_data.is_none() {
                            let read = this.update(cx, |this, cx| {
                                this.start_branches_read(&repo_id, &repo_path, cx)
                            });
                            match read.await {
                                Ok(overview) => remote_branch_data = Some(overview),
                                Err(error) => log::warn!(
                                    "read branches for remotes failed, using cached branch lists {repo_path}: {error}"
                                ),
                            }
                        }

                        let applied = this.update(cx, |this, cx| {
                            if !can_apply(this) {
                                return false;
                            }
                            let items = match &remote_branch_data {
                                Some(overview) => {
                                    let remotes_data: Vec<_> =
                                        remotes.iter().map(remote_data).collect();
                                    build_remote_items(&remotes_data, &branch_data(overview))
                                }
                                None => merge_remote_branch_lists(&remotes, &known_remotes),
                            };
                            this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                tracked::set_remotes_data(repo, items)
                            });
                            true
                        });
                        if applied {
                            outcome.remotes = Some(remotes);
                        } else {
                            stale = true;
                        }
                    }
                    Ok(_) => stale = true,
                    Err(error) => this.update(cx, |this, cx| {
                        this.notify_section_failure(
                            &repo_path,
                            "read remotes failed",
                            &["Error loading remotes", "Could not load remotes"],
                            &error,
                            cx,
                        );
                    }),
                }

                this.update(cx, |this, cx| {
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Remotes, false)
                    });
                });
                if stale {
                    return None;
                }
            }

            let started = matches!(first_read, FirstRead::Tags(_));
            if sections.tags && (started || this.read_with(cx, |this, _| can_apply(this))) {
                let read = match first_read.take() {
                    FirstRead::Tags(read) => read,
                    _ => this.update(cx, |this, cx| {
                        this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                            tracked::set_sidebar_loading(repo, SidebarSection::Tags, true)
                        });
                        this.start_tags_read(&repo_id, &repo_path, cx)
                    }),
                };
                let tags = read.await;
                this.update(cx, |this, cx| {
                    match &tags {
                        Ok(tags) => {
                            if can_apply(this) {
                                this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                    tracked::set_tags_data(repo, tags.clone())
                                });
                            }
                        }
                        Err(error) => this.notify_section_failure(
                            &repo_path,
                            "read tags failed",
                            &["Error loading tags", "Could not load tags"],
                            error,
                            cx,
                        ),
                    }
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Tags, false)
                    });
                });
                outcome.tags = tags.ok();
            }

            let started = matches!(first_read, FirstRead::Stashes(_));
            if sections.stashes && (started || this.read_with(cx, |this, _| can_apply(this))) {
                let read = match first_read.take() {
                    FirstRead::Stashes(read) => read,
                    _ => this.update(cx, |this, cx| {
                        this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                            tracked::set_sidebar_loading(repo, SidebarSection::Stashes, true)
                        });
                        this.start_stashes_read(&repo_id, &repo_path, cx)
                    }),
                };
                let stashes = read.await;
                this.update(cx, |this, cx| {
                    match &stashes {
                        Ok(stashes) => {
                            if can_apply(this) {
                                this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                    tracked::set_stashes_data(repo, stashes.clone())
                                });
                            }
                        }
                        Err(error) => this.notify_section_failure(
                            &repo_path,
                            "read stashes failed",
                            &["Error loading stashes", "Could not load stashes"],
                            error,
                            cx,
                        ),
                    }
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Stashes, false)
                    });
                });
                outcome.stashes = stashes.ok();
            }

            let started = matches!(first_read, FirstRead::Submodules(_));
            if sections.submodules && (started || this.read_with(cx, |this, _| can_apply(this))) {
                let read = match first_read.take() {
                    FirstRead::Submodules(read) => read,
                    _ => this.update(cx, |this, cx| {
                        this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                            tracked::set_sidebar_loading(repo, SidebarSection::Submodules, true)
                        });
                        this.start_submodules_read(&repo_id, &repo_path, cx)
                    }),
                };
                let submodules = read.await;
                this.update(cx, |this, cx| {
                    match &submodules {
                        Ok(submodules) => {
                            if can_apply(this) {
                                this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                    tracked::set_submodules_data(repo, submodules.clone())
                                });
                            }
                        }
                        Err(error) => this.notify_section_failure(
                            &repo_path,
                            "read submodules failed",
                            &["Error loading submodules", "Could not load submodules"],
                            error,
                            cx,
                        ),
                    }
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::Submodules, false)
                    });
                });
                outcome.submodules = submodules.ok();
            }

            let started = matches!(first_read, FirstRead::StarredBranches(_));
            if sections.starred_branches
                && (started || this.read_with(cx, |this, _| can_apply(this)))
            {
                let read = match first_read.take() {
                    FirstRead::StarredBranches(read) => read,
                    _ => this.update(cx, |this, cx| {
                        this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                            tracked::set_sidebar_loading(
                                repo,
                                SidebarSection::StarredBranches,
                                true,
                            )
                        });
                        this.start_starred_branches_read(&repo_id, &repo_path, cx)
                    }),
                };
                let starred = read.await;
                this.update(cx, |this, cx| {
                    match &starred {
                        Ok(starred) => {
                            if can_apply(this) {
                                this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                                    tracked::set_starred_branches(repo, starred.clone())
                                });
                            }
                        }
                        Err(error) => this.notify_section_failure(
                            &repo_path,
                            "read starred branches failed",
                            &["Could not load starred branches"],
                            error,
                            cx,
                        ),
                    }
                    this.update_repository_soon(repo_id.as_str(), cx, |repo| {
                        tracked::set_sidebar_loading(repo, SidebarSection::StarredBranches, false)
                    });
                });
                outcome.starred_branches = starred.ok();
            }

            if let Some(operation) = &sections.operation {
                this.update(cx, |this, cx| {
                    if can_apply(this) {
                        this.record_mutation(repo_id.as_str(), Some(operation), cx);
                    }
                });
            }

            if outcome.status.is_none() {
                outcome.status = result_status;
            }

            Some(outcome)
        })
    }

    /// `ensureRepositorySectionsLoaded`: relee solo las secciones pedidas que
    /// todavía no se han cargado. `None` si no faltaba ninguna.
    pub fn ensure_repository_sections_loaded<'a>(
        &mut self,
        repo: impl Into<RepoKey<'a>>,
        request: SyncRequest,
        cx: &mut Context<Self>,
    ) -> Task<Option<SyncOutcome>> {
        let Some(repository) = self.repositories.repository(repo) else {
            return Task::ready(None);
        };
        let repo_id = repository.id.clone();
        let loaded = repository.sidebar_loaded;

        let missing = SyncRequest {
            status: request.status && repository.status.is_none(),
            branches: request.branches && !loaded.branches,
            remotes: request.remotes && !loaded.remotes,
            tags: request.tags && !loaded.tags,
            stashes: request.stashes && !loaded.stashes,
            submodules: request.submodules && !loaded.submodules,
            changes_count: request.changes_count && !loaded.changes_count,
            starred_branches: request.starred_branches && !loaded.starred_branches,
            result: None,
            operation: None,
        };

        if missing == SyncRequest::default() {
            return Task::ready(None);
        }

        self.sync_repository_read_model(repo_id.as_str(), missing, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_git_query::RemoteRefs;
    use mergeon_model::repository_read_model::RemoteRefs as ModelRemoteRefs;

    #[test]
    fn remotes_keep_the_branch_lists_already_known() {
        let remotes = vec![
            Remote {
                name: "origin".to_string(),
                refs: RemoteRefs {
                    fetch: "git@example.com:demo.git".to_string(),
                    push: "git@example.com:demo.git".to_string(),
                },
            },
            Remote {
                name: "upstream".to_string(),
                refs: RemoteRefs::default(),
            },
        ];
        let existing = vec![RemoteItem {
            name: "origin".to_string(),
            refs: ModelRemoteRefs::default(),
            branches: vec!["main".to_string(), "feature/login".to_string()],
        }];

        let merged = merge_remote_branch_lists(&remotes, &existing);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].branches, ["main", "feature/login"]);
        assert_eq!(merged[0].refs.fetch, "git@example.com:demo.git");
        assert!(merged[1].branches.is_empty());
    }

    #[test]
    fn request_builders_mirror_the_requested_object() {
        let request = SyncRequest::new()
            .status(true)
            .branches(true)
            .operation_or(None, "sequence-action");

        assert!(request.status && request.branches && !request.tags);
        assert_eq!(request.operation.as_deref(), Some("sequence-action"));
        assert_eq!(
            SyncRequest::new()
                .operation_or(Some("rebase"), "sequence-action")
                .operation
                .as_deref(),
            Some("rebase")
        );
    }
}
