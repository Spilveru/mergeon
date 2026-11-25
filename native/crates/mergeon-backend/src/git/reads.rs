//! Canales de lectura: `Result` con el mensaje de `createReadError`.

use std::path::Path;

use mergeon_git_ops::GitIdentity;
use mergeon_git_query::{
    BlameResult, BranchComparison, BranchesOverview, Commit, CommitLogOptions, CommitSearchOptions,
    CompareLimits, CompareMode, ComparisonFiles, ComparisonRevisions, ConflictVersions,
    DiffOptions, DiffPayload, FileAtCommit, FileChange, FileHistoryOptions, FileHistoryPage,
    ImagePair, ImagePairRequest, MergeConflictCheck, OperationState, QueryError, QueryResult,
    ReflogEntry, ReflogOptions, Remote, RemoteConnectionTest, RepositoryInspection,
    RepositorySummary, RevisionComparison, StashEntry, StructuredStatus, Submodule, TagDetails,
    TreeEntries, UncommittedChanges,
};

use super::GitApi;
use super::exec::MISSING_DIRECTORY;

impl GitApi {
    /// Deshacer: `Some(upstream)` si mover `branch` de `from` a `to` deja
    /// fuera commits que ya están en su upstream (luego haría falta un force
    /// push). Lectura, fuera de la cola.
    pub fn undo_published_upstream(
        &self,
        repo_path: impl AsRef<Path>,
        branch: &str,
        from: &str,
        to: &str,
    ) -> Option<String> {
        mergeon_git_ops::undo::published_upstream(repo_path.as_ref(), branch, from, to)
    }

    /// Bytes que ocuparía la copia de un descarte (tamaño de los archivos
    /// implicados), para avisar antes de descartar sin poder deshacer.
    pub fn estimate_discard_backup_size(
        &self,
        repo_path: impl AsRef<Path>,
        file_paths: Option<&[String]>,
    ) -> u64 {
        mergeon_git_ops::undo::estimate_discard_backup_size(repo_path.as_ref(), file_paths)
    }

    /// Canal `git:isValidRepository`.
    pub fn is_valid_repository(&self, repo_path: impl AsRef<Path>) -> bool {
        mergeon_git_query::is_valid_repository(repo_path.as_ref())
    }

    /// Canal `git:inspectRepository`. Nunca falla: el motivo va en `kind`.
    pub fn inspect_repository(&self, repo_path: impl AsRef<Path>) -> RepositoryInspection {
        mergeon_git_query::inspect_repository(repo_path.as_ref())
    }

    /// Canal `git:getCommits`.
    pub fn get_commits(
        &self,
        repo_path: impl AsRef<Path>,
        options: &CommitLogOptions,
    ) -> QueryResult<Vec<Commit>> {
        mergeon_git_query::get_commits(repo_path.as_ref(), options)
    }

    /// Canal `git:searchCommits`: busca en el historial completo (mensaje,
    /// autor o hash).
    pub fn search_commits(
        &self,
        repo_path: impl AsRef<Path>,
        options: &CommitSearchOptions,
    ) -> QueryResult<Vec<Commit>> {
        mergeon_git_query::search_commits(repo_path.as_ref(), options)
    }

    /// Canal `git:resolveRef`: hash del commit al que apunta un ref (rama
    /// local/remota o tag).
    pub fn resolve_ref(&self, repo_path: impl AsRef<Path>, reference: &str) -> QueryResult<String> {
        const PREFIX: &str = "Failed to resolve the ref";
        let repo_path = repo_path.as_ref();
        if !repo_path.is_dir() {
            return Err(QueryError::read(PREFIX, MISSING_DIRECTORY));
        }
        mergeon_git_ops::resolve_ref_to_commit(repo_path, reference)
            .map_err(|error| QueryError::read(PREFIX, error.message))
    }

    /// Canal `git:getIdentity`: identidad de commit (`user.name` /
    /// `user.email`, local y global). Como en el original, una clave que no se
    /// puede leer llega como `None`, así que en la práctica no falla.
    pub fn get_identity(&self, repo_path: impl AsRef<Path>) -> QueryResult<GitIdentity> {
        Ok(mergeon_git_ops::get_git_identity(repo_path.as_ref()))
    }

    /// Canal `git:getReflog`: reflog de HEAD (red de seguridad).
    pub fn get_reflog(
        &self,
        repo_path: impl AsRef<Path>,
        options: &ReflogOptions,
    ) -> QueryResult<Vec<ReflogEntry>> {
        mergeon_git_query::get_reflog(repo_path.as_ref(), options)
    }

    /// Canal `git:getBranches`.
    pub fn get_branches(&self, repo_path: impl AsRef<Path>) -> QueryResult<BranchesOverview> {
        mergeon_git_query::get_branches(repo_path.as_ref())
    }

    /// Canal `git:getMergedBranches` (sin original): ramas locales que la
    /// limpieza puede proponer borrar, contra `base` o la rama por defecto.
    pub fn get_merged_branches(
        &self,
        repo_path: impl AsRef<Path>,
        base: Option<&str>,
    ) -> QueryResult<mergeon_git_query::MergedBranchesReport> {
        mergeon_git_query::get_merged_branches(repo_path.as_ref(), base)
    }

    /// Canal `git:getRemotes`.
    pub fn get_remotes(&self, repo_path: impl AsRef<Path>) -> QueryResult<Vec<Remote>> {
        mergeon_git_query::get_remotes(repo_path.as_ref())
    }

    /// Canal `git:getSubmodules`.
    pub fn get_submodules(&self, repo_path: impl AsRef<Path>) -> QueryResult<Vec<Submodule>> {
        mergeon_git_query::get_submodules(repo_path.as_ref())
    }

    /// Los worktrees del repositorio (`git worktree list --porcelain -z`).
    pub fn get_worktrees(
        &self,
        repo_path: impl AsRef<Path>,
    ) -> QueryResult<Vec<mergeon_git_query::Worktree>> {
        mergeon_git_query::get_worktrees(repo_path.as_ref())
    }

    /// Si el worktree de `path` tiene cambios sin commitear (barato: corta en
    /// el primer byte de `git status`).
    pub fn worktree_has_changes(&self, path: impl AsRef<Path>) -> QueryResult<bool> {
        mergeon_git_query::worktree_has_changes(path.as_ref())
    }

    /// Canal `git:getRepositorySummary`.
    pub fn get_repository_summary(
        &self,
        repo_path: impl AsRef<Path>,
    ) -> QueryResult<RepositorySummary> {
        mergeon_git_query::get_repository_summary(repo_path.as_ref())
    }

    /// Resumen nuevo que reaprovecha el recuento de commits y las fechas de
    /// `previous` si HEAD no se movió (ver `refresh_repository_summary`).
    pub fn refresh_repository_summary(
        &self,
        repo_path: impl AsRef<Path>,
        previous: Option<&RepositorySummary>,
    ) -> QueryResult<RepositorySummary> {
        mergeon_git_query::refresh_repository_summary(repo_path.as_ref(), previous)
    }

    /// Canal `git:testRemoteConnection`. Nunca falla: el motivo va en `message`.
    pub fn test_remote_connection(
        &self,
        repo_path: impl AsRef<Path>,
        url: &str,
    ) -> RemoteConnectionTest {
        mergeon_git_query::test_remote_connection(repo_path.as_ref(), url)
    }

    /// Canal `git:getStatus`.
    pub fn get_status(&self, repo_path: impl AsRef<Path>) -> QueryResult<StructuredStatus> {
        mergeon_git_query::get_status(repo_path.as_ref())
    }

    /// Canal `git:getOperationState`: merge, rebase, cherry-pick o revert en curso.
    pub fn get_operation_state(&self, repo_path: impl AsRef<Path>) -> QueryResult<OperationState> {
        mergeon_git_query::get_operation_state(repo_path.as_ref())
    }

    /// Canal `git:getTags` (del más reciente al más antiguo).
    pub fn get_tags(&self, repo_path: impl AsRef<Path>) -> QueryResult<Vec<String>> {
        mergeon_git_query::get_tags(repo_path.as_ref())
    }

    /// Canal `git:getTagDetails`.
    pub fn get_tag_details(
        &self,
        repo_path: impl AsRef<Path>,
        tag_name: &str,
    ) -> QueryResult<TagDetails> {
        mergeon_git_query::get_tag_details(repo_path.as_ref(), tag_name)
    }

    /// Canal `git:checkMergeConflicts`: predice si fusionar `source` en
    /// `target` daría conflicto, sin tocar el working tree.
    pub fn check_merge_conflicts(
        &self,
        repo_path: impl AsRef<Path>,
        source: &str,
        target: &str,
    ) -> QueryResult<MergeConflictCheck> {
        mergeon_git_query::check_merge_conflicts(repo_path.as_ref(), source, target)
    }

    /// Canal `git:getStashes`.
    pub fn get_stashes(&self, repo_path: impl AsRef<Path>) -> QueryResult<Vec<StashEntry>> {
        mergeon_git_query::get_stashes(repo_path.as_ref())
    }

    /// Canal `git:getStashFiles`.
    pub fn get_stash_files(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
    ) -> QueryResult<Vec<FileChange>> {
        mergeon_git_query::get_stash_files(repo_path.as_ref(), stash_ref)
    }

    /// Canal `git:getStashDiff`.
    pub fn get_stash_diff(
        &self,
        repo_path: impl AsRef<Path>,
        stash_ref: &str,
        file_path: &str,
        options: &DiffOptions,
    ) -> QueryResult<DiffPayload> {
        mergeon_git_query::get_stash_diff(repo_path.as_ref(), stash_ref, file_path, options)
    }

    /// Canal `git:compareBranches`.
    pub fn compare_branches(
        &self,
        repo_path: impl AsRef<Path>,
        branch1: &str,
        branch2: &str,
    ) -> QueryResult<BranchComparison> {
        mergeon_git_query::compare_branches(repo_path.as_ref(), branch1, branch2)
    }

    /// Comparación navegable de dos revisiones (sin canal en el original):
    /// las fija a su SHA y lista commits y archivos según `mode`.
    pub fn compare_revisions(
        &self,
        repo_path: impl AsRef<Path>,
        base: &str,
        head: &str,
        mode: CompareMode,
        limits: CompareLimits,
    ) -> QueryResult<RevisionComparison> {
        mergeon_git_query::compare_revisions(repo_path.as_ref(), base, head, mode, limits)
    }

    /// Los archivos de una comparación ya fijada, en otro modo.
    pub fn get_comparison_files(
        &self,
        repo_path: impl AsRef<Path>,
        revisions: &ComparisonRevisions,
        mode: CompareMode,
        limit: usize,
    ) -> QueryResult<ComparisonFiles> {
        mergeon_git_query::get_comparison_files(repo_path.as_ref(), revisions, mode, limit)
    }

    /// Parche de un archivo de una comparación.
    pub fn get_comparison_diff(
        &self,
        repo_path: impl AsRef<Path>,
        revisions: &ComparisonRevisions,
        mode: CompareMode,
        file_path: &str,
        old_path: Option<&str>,
        options: &DiffOptions,
    ) -> QueryResult<DiffPayload> {
        mergeon_git_query::get_comparison_diff(
            repo_path.as_ref(),
            revisions,
            mode,
            file_path,
            old_path,
            options,
        )
    }

    /// Canal `git:hasUncommittedChanges`.
    pub fn has_uncommitted_changes(
        &self,
        repo_path: impl AsRef<Path>,
    ) -> QueryResult<UncommittedChanges> {
        mergeon_git_query::has_uncommitted_changes(repo_path.as_ref())
    }

    /// Canal `git:blame`.
    pub fn blame(&self, repo_path: impl AsRef<Path>, file_path: &str) -> QueryResult<BlameResult> {
        mergeon_git_query::blame(repo_path.as_ref(), file_path)
    }

    /// Sin canal en el original: blame del archivo en una revisión
    /// ("Blame previous revision").
    pub fn blame_at_revision(
        &self,
        repo_path: impl AsRef<Path>,
        revision: &str,
        file_path: &str,
    ) -> QueryResult<BlameResult> {
        mergeon_git_query::blame_at_revision(repo_path.as_ref(), revision, file_path)
    }

    /// Sin canal en el original: historial de un archivo siguiendo sus
    /// renombres, por páginas.
    pub fn get_file_history(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
        options: &FileHistoryOptions,
    ) -> QueryResult<FileHistoryPage> {
        mergeon_git_query::get_file_history(repo_path.as_ref(), file_path, options)
    }

    /// Sin canal en el original: parche de un archivo en una de sus
    /// revisiones, con la ruta anterior si la revisión lo renombró.
    pub fn get_file_revision_diff(
        &self,
        repo_path: impl AsRef<Path>,
        commit_hash: &str,
        file_path: &str,
        previous_path: Option<&str>,
        options: &DiffOptions,
    ) -> QueryResult<DiffPayload> {
        mergeon_git_query::get_file_revision_diff(
            repo_path.as_ref(),
            commit_hash,
            file_path,
            previous_path,
            options,
        )
    }

    /// Canal `git:readFile`: contenido de un archivo del working tree (para
    /// resolver conflictos).
    pub fn read_file(&self, repo_path: impl AsRef<Path>, file_path: &str) -> QueryResult<String> {
        mergeon_git_query::read_file(repo_path.as_ref(), file_path)
    }

    /// Las tres versiones de un archivo en conflicto (etapas `:1:`, `:2:` y
    /// `:3:` del índice) y su contenido en el working tree.
    pub fn read_conflict_versions(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
    ) -> QueryResult<ConflictVersions> {
        mergeon_git_query::read_conflict_versions(repo_path.as_ref(), file_path)
    }

    /// Canal `git:getDiff`.
    pub fn get_diff(
        &self,
        repo_path: impl AsRef<Path>,
        file_path: &str,
        staged: bool,
        is_untracked: bool,
        options: &DiffOptions,
    ) -> QueryResult<DiffPayload> {
        mergeon_git_query::get_diff(repo_path.as_ref(), file_path, staged, is_untracked, options)
    }

    /// Sin canal en el original: las dos versiones de una imagen cambiada
    /// (antes y después), con tope de tamaño.
    pub fn read_image_pair(
        &self,
        repo_path: impl AsRef<Path>,
        request: &ImagePairRequest,
    ) -> QueryResult<ImagePair> {
        mergeon_git_query::read_image_pair(repo_path.as_ref(), request)
    }

    /// Canal `git:getCommitDiff`.
    pub fn get_commit_diff(
        &self,
        repo_path: impl AsRef<Path>,
        commit_hash: &str,
        file_path: &str,
        options: &DiffOptions,
    ) -> QueryResult<DiffPayload> {
        mergeon_git_query::get_commit_diff(repo_path.as_ref(), commit_hash, file_path, options)
    }

    /// Canal `git:getCommitFiles`.
    pub fn get_commit_files(
        &self,
        repo_path: impl AsRef<Path>,
        commit_hash: &str,
    ) -> QueryResult<Vec<FileChange>> {
        mergeon_git_query::get_commit_files(repo_path.as_ref(), commit_hash)
    }

    /// Canal `git:getTreeEntries`: un nivel del árbol del repositorio en ese
    /// commit. `directory` vacío es la raíz.
    pub fn get_tree_entries(
        &self,
        repo_path: impl AsRef<Path>,
        commit_hash: &str,
        directory: &str,
    ) -> QueryResult<TreeEntries> {
        mergeon_git_query::get_tree_entries(repo_path.as_ref(), commit_hash, directory)
    }

    /// Canal `git:getFileAtCommit`: el contenido de un archivo tal como estaba
    /// en ese commit.
    pub fn get_file_at_commit(
        &self,
        repo_path: impl AsRef<Path>,
        commit_hash: &str,
        file_path: &str,
    ) -> QueryResult<FileAtCommit> {
        mergeon_git_query::get_file_at_commit(repo_path.as_ref(), commit_hash, file_path)
    }
}
