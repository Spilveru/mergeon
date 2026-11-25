# Port del backend de Mergeon a Rust

Mergeon se está reescribiendo de Electron + Vue a Rust + GPUI. La app Electron
(directorios `electron/` y `src/` en la raíz del repo) es la **especificación**:
no se modifica, se porta. Este documento fija las convenciones de los crates de
backend (todo lo que hoy vive en `electron/`).

## Crates

| Crate | Origen | Contenido |
|---|---|---|
| `mergeon-gitcmd` | parte de `git-operations.js`, `git-operation-context.js` | Ejecutor de `git`, contexto de operación, estado estructurado. **Ya escrito: leerlo antes de empezar.** |
| `mergeon-git-ops` | resto de `git-operations.js` | Operaciones que mutan el repo. |
| `mergeon-git-query` | `git-handlers.js` (lecturas) y servicios pequeños | Lecturas. |
| `mergeon-runtime` | `repository-mutation-gate.js`, `repository-status-watch.js` | Cola de mutaciones y watcher. |
| `mergeon-store` | `store.js`, `storage-handlers.js` | Configuración persistente. |
| `mergeon-github` | `github-auth-client.js`, `github-repo-url.js`, `github-handlers.js` | Cliente GitHub. |

Cada crate lo escribe un agente distinto **a la vez**. Reglas para no pisarse:

- Tocar solo los archivos del crate propio (`crates/<crate>/**`). No editar el
  `Cargo.toml` raíz ni otros crates. Si falta algo en `mergeon-gitcmd`, no
  editarlo: resolverlo dentro del crate propio y dejarlo anotado en el informe final.
- Las dependencias nuevas van en el `Cargo.toml` del crate propio con versión
  explícita (no en `[workspace.dependencies]`). Pocas y conocidas.
- Compilar y probar con un directorio de salida propio para no bloquear a los demás:

  ```bash
  export PATH="$HOME/.cargo/bin:$PATH"
  cd native
  CARGO_TARGET_DIR=$PWD/target-<crate> cargo test -p <crate>
  CARGO_TARGET_DIR=$PWD/target-<crate> cargo clippy -p <crate> --all-targets -- -D warnings
  cargo fmt -p <crate>
  ```

  No correr `cargo build`/`cargo test` sin `-p` ni sin `CARGO_TARGET_DIR`: el
  workspace incluye la app GPUI, que tarda mucho en compilar y usa `target/`.

## Convenciones

- **API síncrona y bloqueante.** Nada de async ni runtimes. La UI llama desde un
  hilo de fondo. La concurrencia interna se hace con `std::thread`.
- **Fidelidad al original.** Mismos comandos de git, mismos argumentos, mismo
  orden, mismos mensajes al usuario (los textos en inglés se copian literales),
  mismos `code` / `outcome` / `operation`. Si el JS tiene un comentario que
  explica un porqué, se conserva (en español, como en el original).
- **Formas de datos.** Structs con `serde::{Serialize, Deserialize}` y los mismos
  nombres de campo que el objeto JS (`#[serde(rename_all = "camelCase")]`; los
  que en JS van en snake_case, como `not_added`, se renombran a mano). Los campos
  que en JS pueden ser `null`/`undefined` son `Option<_>`. Los enums de strings
  (`outcome`, `code`...) son enums de Rust con `as_str()`.
- **Contrato de errores (dual, intencional, igual que en `electron/ipc/handler-utils.js`):**
  - Lecturas: `Result<T, E>`; el error lleva el mismo mensaje que construía
    `createReadError(prefix, error)` → `"<prefix>: <mensaje>"`.
  - Mutaciones: devuelven **siempre** el objeto de resultado estructurado
    (`success`, `operation`, `outcome`, `code`, `message`, ...), nunca `Err`.
- **Sin pánicos en código de librería**: nada de `unwrap()`/`expect()` salvo
  invariantes internos evidentes (locks, regex constantes).
- **Identificadores en inglés, comentarios en español**, como el código actual.
  Comentarios solo donde aporten un porqué.
- **Multiplataforma.** Debe compilar y funcionar en Linux, Windows y macOS. Rutas
  con `Path`/`PathBuf`; lo específico de una plataforma detrás de `cfg`.

## Semántica de simple-git que hay que conservar

La app actual usa `simple-git`. `mergeon_gitcmd::Git` la reemplaza:

- `git.raw([...])` → `Git::raw`: falla solo si git sale ≠ 0 **y** escribió en
  stderr; el mensaje del error es `stdout + stderr` sin recortar. Salida ≠ 0 con
  stderr vacío se resuelve con stdout. Para decidir por código de salida usar
  `Git::exec`, que devuelve stdout, stderr y código.
- `git.revparse([...])` → `Git::revparse` (recortado).
- `git.status()` → `Git::status` (mismo parser que simple-git).
- `getGit(repoPath, { config: [...] })` → `Git::new(path).with_config([...])`.
- `git.env(...)` → `Git::with_env`.
- Los métodos de alto nivel de simple-git (`git.merge`, `git.pull`, `git.push`,
  `git.commit`, `git.branch`, `git.log`, `git.diff`, `git.add`, `git.reset`,
  `git.checkout`, `git.stash`, `git.tag`, `git.fetch`, `git.clone`...) no existen
  aquí: hay que ejecutar el mismo comando que arma simple-git y parsear lo que
  el código de Mergeon realmente usa del resultado. El fuente está en
  `node_modules/simple-git/dist/esm/index.js` (buscar `// src/lib/tasks/<x>.ts`
  y `// src/lib/parsers/`); leerlo para conocer los argumentos exactos y los
  errores especiales (p. ej. `git.merge` rechaza con un `GitResponseError` cuando
  hay conflictos, `git.pull`/`git.push` parsean stderr).
- El contexto de operación (`runWithGitOperationContext`, `getGitOperationContext`)
  es `mergeon_gitcmd::run_with_operation_context` / `current_operation_context`:
  un thread-local, así que un `Git` hay que crearlo en el hilo que ejecuta la
  operación. `getGitOperationControlCode(error)` es `operation_control_code`.

## Tests

- Portar **todos** los tests de vitest del área (`*.test.js` junto al fuente),
  conservando la intención de cada caso. Donde el JS mockea `simple-git`, preferir
  un repositorio git real en un directorio temporal (`tempfile`), con un origin
  bare local cuando haga falta un remoto (ver `e2e/helpers/gitFixtures.js`).
  Solo usar dobles de prueba cuando el caso no se pueda provocar con git real.
- Los repos de prueba deben fijar identidad (`user.name`, `user.email`),
  `commit.gpgsign=false` y rama inicial `main`, sin depender de la configuración
  global de la máquina.
- Los tests deben pasar en paralelo (`cargo test` usa varios hilos) y sin red.

## Informe final

Al terminar, el agente entrega: la API pública del crate (firmas), qué archivos
JS quedaron portados y cuáles no, tests portados/omitidos con el motivo,
resultado de `cargo test` y `cargo clippy`, y cualquier diferencia de
comportamiento deliberada respecto del original.

## Crate `mergeon-model` (lógica pura del renderer)

Además del backend, la lógica sin UI que hoy vive en `src/lib`, `src/utils` y
partes de `src/stores` / `src/composables` se porta a `mergeon-model` con las
mismas convenciones: funciones puras, mismos nombres en snake_case, mismas
formas de datos y todos los tests de vitest portados. Este crate no depende de
GPUI.
