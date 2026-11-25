//! "Copy GitHub link": la página de GitHub de un commit
//! (`https://github.com/<owner>/<repo>/commit/<sha>`) o de un archivo tal
//! como estaba en él (`…/blob/<sha>/<ruta>`, con `#L<n>` si se pide una
//! línea).
//!
//! El repositorio de GitHub sale del remoto del upstream de la rama actual o,
//! si ese no es de GitHub, de `origin`. La URL del remoto se convierte con la
//! misma regla que el enlace de pull request del sidebar
//! (`normalize_github_remote_url`), más la forma `ssh://git@github.com/…`
//! que entiende `mergeon_github`.

use mergeon_app::OpenRepository;
use mergeon_github::parse_github_repository_url;
use mergeon_model::repository_read_model::{BranchItem, RemoteItem};
use mergeon_view_sidebar::logic::{
    encode_uri_component, normalize_github_remote_url, normalize_remote_url,
};

/// La URL web (`https://github.com/<owner>/<repo>`) de un remoto de GitHub,
/// https o ssh, con o sin `.git`. `None` si el remoto no es de github.com.
pub fn github_repository_url(remote_url: &str) -> Option<String> {
    let url = normalize_remote_url(remote_url);
    normalize_github_remote_url(&url).or_else(|| {
        parse_github_repository_url(&url)
            .map(|reference| format!("https://github.com/{}/{}", reference.owner, reference.repo))
    })
}

/// La URL web de GitHub del repositorio: la del remoto del upstream de
/// `current_branch` y, si no lo hay o no es de GitHub, la de `origin`.
pub fn repository_github_url(
    current_branch: Option<&str>,
    branches: &[BranchItem],
    remotes: &[RemoteItem],
) -> Option<String> {
    let upstream = current_branch
        .filter(|name| !name.is_empty())
        .and_then(|name| {
            branches
                .iter()
                .find(|branch| !branch.is_remote && branch.name == name)
        })
        .and_then(|branch| branch.upstream.as_deref())
        .filter(|upstream| !upstream.is_empty());
    // `origin/feature/x`: el remoto es el de nombre más largo que encaje (un
    // remoto puede llevar `/` en el nombre).
    let upstream_remote = upstream.and_then(|upstream| {
        remotes
            .iter()
            .filter(|remote| {
                upstream
                    .strip_prefix(remote.name.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|remote| remote.name.len())
    });
    let origin = remotes.iter().find(|remote| remote.name == "origin");

    [upstream_remote, origin]
        .into_iter()
        .flatten()
        .find_map(remote_github_url)
}

/// [`repository_github_url`] de un repositorio abierto.
pub(crate) fn open_repository_github_url(repository: &OpenRepository) -> Option<String> {
    repository_github_url(
        repository.current_branch.as_deref(),
        &repository.branches,
        &repository.remotes,
    )
}

fn remote_github_url(remote: &RemoteItem) -> Option<String> {
    [&remote.refs.fetch, &remote.refs.push]
        .into_iter()
        .filter(|url| !url.is_empty())
        .find_map(|url| github_repository_url(url))
}

/// `https://github.com/<owner>/<repo>/commit/<sha>`.
pub fn github_commit_url(repository_url: &str, sha: &str) -> String {
    format!("{repository_url}/commit/{sha}")
}

/// `https://github.com/<owner>/<repo>/blob/<sha>/<ruta>[#L<n>]`. Cada tramo
/// de la ruta va codificado (espacios, `#`, `?`…); las barras se quedan.
pub fn github_blob_url(repository_url: &str, sha: &str, path: &str, line: Option<u32>) -> String {
    let path = path
        .trim_start_matches('/')
        .split('/')
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/");
    let anchor = line.map(|line| format!("#L{line}")).unwrap_or_default();
    format!("{repository_url}/blob/{sha}/{path}{anchor}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::repository_read_model::RemoteRefs;

    fn remote(name: &str, url: &str) -> RemoteItem {
        RemoteItem {
            name: name.to_string(),
            refs: RemoteRefs {
                fetch: url.to_string(),
                push: url.to_string(),
            },
            branches: Vec::new(),
        }
    }

    fn branch(name: &str, upstream: Option<&str>) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            upstream: upstream.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn turns_https_and_ssh_remotes_with_or_without_git_into_the_web_url() {
        for url in [
            "https://github.com/octocat/hello-world.git",
            "https://github.com/octocat/hello-world",
            "https://github.com/octocat/hello-world/",
            "git@github.com:octocat/hello-world.git",
            "git@github.com:octocat/hello-world",
            "ssh://git@github.com/octocat/hello-world.git",
            "ssh://git@github.com/octocat/hello-world",
            "  https://GitHub.com/octocat/hello-world.git  ",
        ] {
            assert_eq!(
                github_repository_url(url).as_deref(),
                Some("https://github.com/octocat/hello-world"),
                "{url}"
            );
        }
    }

    #[test]
    fn other_hosts_and_local_paths_are_not_github() {
        for url in [
            "",
            "/srv/git/demo.git",
            "file:///srv/git/demo.git",
            "https://gitlab.com/octocat/hello-world.git",
            "git@bitbucket.org:octocat/hello-world.git",
            "https://github.com/octocat",
        ] {
            assert_eq!(github_repository_url(url), None, "{url}");
        }
    }

    #[test]
    fn builds_commit_and_blob_urls() {
        let repo = "https://github.com/octocat/hello-world";
        assert_eq!(
            github_commit_url(repo, "0123abcd"),
            "https://github.com/octocat/hello-world/commit/0123abcd"
        );
        assert_eq!(
            github_blob_url(repo, "0123abcd", "src/main.rs", None),
            "https://github.com/octocat/hello-world/blob/0123abcd/src/main.rs"
        );
        assert_eq!(
            github_blob_url(repo, "0123abcd", "docs/my notes#1.md", Some(42)),
            "https://github.com/octocat/hello-world/blob/0123abcd/docs/my%20notes%231.md#L42"
        );
    }

    #[test]
    fn prefers_the_upstream_remote_then_origin() {
        let remotes = [
            remote("origin", "git@github.com:me/fork.git"),
            remote("upstream", "https://github.com/team/project.git"),
            remote("mirror", "https://gitlab.com/team/project.git"),
        ];
        let branches = [
            branch("main", Some("upstream/main")),
            branch("topic", Some("mirror/topic")),
            branch("local", None),
        ];
        let url = |current| repository_github_url(Some(current), &branches, &remotes);

        assert_eq!(
            url("main").as_deref(),
            Some("https://github.com/team/project")
        );
        // El upstream no es de GitHub: se usa origin.
        assert_eq!(url("topic").as_deref(), Some("https://github.com/me/fork"));
        // Sin upstream, origin.
        assert_eq!(url("local").as_deref(), Some("https://github.com/me/fork"));
        assert_eq!(
            repository_github_url(None, &branches, &remotes).as_deref(),
            Some("https://github.com/me/fork")
        );
    }

    #[test]
    fn without_a_github_remote_there_is_no_link() {
        let remotes = [remote("origin", "/srv/git/demo.git")];
        assert_eq!(
            repository_github_url(
                Some("main"),
                &[branch("main", Some("origin/main"))],
                &remotes
            ),
            None
        );
        assert_eq!(repository_github_url(Some("main"), &[], &[]), None);
    }
}
