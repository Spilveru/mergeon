//! Editores externos: el catálogo de editores conocidos, cómo se encuentran en
//! el sistema y con qué argumentos se les pide abrir un archivo en una línea.
//!
//! Aquí solo hay datos y funciones puras; lanzar el proceso es cosa de
//! [`crate::Shell::open_in_editor`], que pasa por el `ProcessLauncher`.

/// Cómo pide cada editor abrir un archivo en una línea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    /// VS Code y derivados (Cursor, VSCodium): `--goto path:line:column`.
    Goto,
    /// Zed y Sublime Text: `path:line:column`.
    Suffix,
    /// IDEs de JetBrains: `--line N path`.
    JetBrains,
    /// Notepad++: `-nN path`.
    NotepadPlusPlus,
}

/// Un editor del catálogo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorDefinition {
    /// Lo que se guarda en `settings.externalEditor.editor`.
    pub id: &'static str,
    pub name: &'static str,
    /// Nombres del ejecutable en el `PATH`, por preferencia (sin extensión).
    pub commands: &'static [&'static str],
    pub line_style: LineStyle,
    /// Rutas típicas de instalación en Windows: la variable de entorno de la
    /// carpeta base y el resto de la ruta (con `\`).
    pub windows_paths: &'static [(&'static str, &'static str)],
    /// Rutas típicas de instalación en macOS (el lanzador de línea de órdenes
    /// que trae el `.app`, por si no se instaló en el `PATH`).
    pub mac_paths: &'static [&'static str],
    /// Solo existe en Windows (Notepad++).
    pub windows_only: bool,
}

impl EditorDefinition {
    const fn new(
        id: &'static str,
        name: &'static str,
        commands: &'static [&'static str],
        line_style: LineStyle,
    ) -> Self {
        Self {
            id,
            name,
            commands,
            line_style,
            windows_paths: &[],
            mac_paths: &[],
            windows_only: false,
        }
    }

    const fn windows(mut self, paths: &'static [(&'static str, &'static str)]) -> Self {
        self.windows_paths = paths;
        self
    }

    const fn mac(mut self, paths: &'static [&'static str]) -> Self {
        self.mac_paths = paths;
        self
    }

    const fn windows_only(mut self) -> Self {
        self.windows_only = true;
        self
    }
}

const LOCAL_APP_DATA: &str = "LOCALAPPDATA";
const PROGRAM_FILES: &str = "ProgramFiles";
const PROGRAM_FILES_X86: &str = "ProgramFiles(x86)";

/// Los editores que se detectan, en el orden en que "Automatic" los prueba.
pub const EDITORS: &[EditorDefinition] = &[
    EditorDefinition::new("vscode", "Visual Studio Code", &["code"], LineStyle::Goto)
        .windows(&[
            (LOCAL_APP_DATA, r"Programs\Microsoft VS Code\Code.exe"),
            (PROGRAM_FILES, r"Microsoft VS Code\Code.exe"),
        ])
        .mac(&["/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"]),
    EditorDefinition::new("cursor", "Cursor", &["cursor"], LineStyle::Goto)
        .windows(&[(LOCAL_APP_DATA, r"Programs\cursor\Cursor.exe")])
        .mac(&["/Applications/Cursor.app/Contents/Resources/app/bin/cursor"]),
    EditorDefinition::new("zed", "Zed", &["zed", "zeditor"], LineStyle::Suffix)
        .windows(&[
            (LOCAL_APP_DATA, r"Programs\Zed\bin\zed.exe"),
            (LOCAL_APP_DATA, r"Programs\Zed\Zed.exe"),
        ])
        .mac(&["/Applications/Zed.app/Contents/MacOS/cli"]),
    EditorDefinition::new("sublime", "Sublime Text", &["subl"], LineStyle::Suffix)
        .windows(&[
            (PROGRAM_FILES, r"Sublime Text\subl.exe"),
            (PROGRAM_FILES, r"Sublime Text 3\subl.exe"),
        ])
        .mac(&["/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl"]),
    EditorDefinition::new(
        "vscode-insiders",
        "Visual Studio Code - Insiders",
        &["code-insiders"],
        LineStyle::Goto,
    )
    .windows(&[
        (
            LOCAL_APP_DATA,
            r"Programs\Microsoft VS Code Insiders\Code - Insiders.exe",
        ),
        (
            PROGRAM_FILES,
            r"Microsoft VS Code Insiders\Code - Insiders.exe",
        ),
    ])
    .mac(&[
        "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders",
    ]),
    EditorDefinition::new("vscodium", "VSCodium", &["codium"], LineStyle::Goto)
        .windows(&[
            (LOCAL_APP_DATA, r"Programs\VSCodium\VSCodium.exe"),
            (PROGRAM_FILES, r"VSCodium\VSCodium.exe"),
        ])
        .mac(&["/Applications/VSCodium.app/Contents/Resources/app/bin/codium"]),
    // JetBrains: los lanzadores que instala Toolbox (o "Create command-line
    // launcher") en el PATH.
    EditorDefinition::new("intellij", "IntelliJ IDEA", &["idea"], LineStyle::JetBrains),
    EditorDefinition::new("pycharm", "PyCharm", &["pycharm"], LineStyle::JetBrains),
    EditorDefinition::new("webstorm", "WebStorm", &["webstorm"], LineStyle::JetBrains),
    EditorDefinition::new("phpstorm", "PhpStorm", &["phpstorm"], LineStyle::JetBrains),
    EditorDefinition::new("goland", "GoLand", &["goland"], LineStyle::JetBrains),
    EditorDefinition::new("clion", "CLion", &["clion"], LineStyle::JetBrains),
    EditorDefinition::new(
        "rustrover",
        "RustRover",
        &["rustrover"],
        LineStyle::JetBrains,
    ),
    EditorDefinition::new("rider", "Rider", &["rider"], LineStyle::JetBrains),
    EditorDefinition::new("rubymine", "RubyMine", &["rubymine"], LineStyle::JetBrains),
    EditorDefinition::new(
        "notepad++",
        "Notepad++",
        &["notepad++"],
        LineStyle::NotepadPlusPlus,
    )
    .windows(&[
        (PROGRAM_FILES, r"Notepad++\notepad++.exe"),
        (PROGRAM_FILES_X86, r"Notepad++\notepad++.exe"),
    ])
    .windows_only(),
];

/// El editor del catálogo con ese id o con ese ejecutable (`"code"` vale
/// por VS Code: así se entiende también un ajuste escrito a mano).
pub fn editor_definition(id_or_command: &str) -> Option<&'static EditorDefinition> {
    EDITORS
        .iter()
        .find(|editor| editor.id == id_or_command)
        .or_else(|| {
            EDITORS
                .iter()
                .find(|editor| editor.commands.contains(&id_or_command))
        })
}

/// Un editor del catálogo que está instalado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEditor {
    pub id: String,
    pub name: String,
    /// Ruta completa del ejecutable que se lanzará.
    pub executable: String,
}

/// Lo que hay que hacer para abrir un archivo en el editor elegido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorLaunch {
    /// Lanzar `program` con `args` (sin pasar por una shell).
    Command {
        /// Nombre para los avisos ("Visual Studio Code", o el comando propio).
        name: String,
        program: String,
        args: Vec<String>,
    },
    /// Abrirlo con el programa por defecto del sistema.
    SystemDefault,
}

/// Con qué se abrió un archivo en [`crate::Shell::open_in_editor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorOpened {
    /// Con el editor elegido o detectado.
    Editor { name: String },
    /// Con el programa por defecto del sistema. `fallback`: no porque se
    /// eligiera así, sino porque no hay editor elegido ni se encontró ninguno.
    SystemDefault { fallback: bool },
}

/// Columna que se pide al editor: Mergeon no sigue el cursor dentro de la
/// línea, así que siempre es la primera.
const COLUMN: u32 = 1;

/// Argumentos para abrir `path` (ya con los separadores de la plataforma) en
/// un editor del catálogo. Sin línea, solo la ruta: así también se abre una
/// carpeta (el repositorio entero).
pub fn editor_arguments(style: LineStyle, path: &str, line: Option<u32>) -> Vec<String> {
    let Some(line) = line else {
        return vec![path.to_string()];
    };
    match style {
        LineStyle::Goto => vec!["--goto".to_string(), format!("{path}:{line}:{COLUMN}")],
        LineStyle::Suffix => vec![format!("{path}:{line}:{COLUMN}")],
        LineStyle::JetBrains => vec!["--line".to_string(), line.to_string(), path.to_string()],
        LineStyle::NotepadPlusPlus => vec![format!("-n{line}"), path.to_string()],
    }
}

/// Parte la plantilla de argumentos de un editor propio como lo haría una
/// shell sencilla, pero sin ejecutar ninguna: espacios separan, y las
/// comillas dobles o simples agrupan (`"{path}"` con espacios queda en un
/// solo argumento). La barra invertida no escapa nada: en Windows es el
/// separador de rutas.
pub fn split_arguments(template: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_arg = false;
    for character in template.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => {
                quote = Some(character);
                in_arg = true;
            }
            None if character.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            None => {
                current.push(character);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    args
}

/// Marcadores de la plantilla de un editor propio.
pub const PATH_MARKER: &str = "{path}";
pub const LINE_MARKER: &str = "{line}";
pub const COLUMN_MARKER: &str = "{column}";

/// Argumentos de un editor propio: la plantilla partida y con los marcadores
/// sustituidos en cada argumento (una ruta con espacios sigue siendo uno
/// solo). Sin línea, `{line}` y `{column}` valen 1. Si la plantilla no lleva
/// `{path}`, la ruta va al final.
pub fn custom_arguments(template: &str, path: &str, line: Option<u32>) -> Vec<String> {
    let line = line.unwrap_or(1).to_string();
    let column = COLUMN.to_string();
    let markers = [
        (PATH_MARKER, path),
        (LINE_MARKER, line.as_str()),
        (COLUMN_MARKER, column.as_str()),
    ];
    let mut args: Vec<String> = split_arguments(template)
        .into_iter()
        .map(|arg| substitute(&arg, &markers))
        .collect();
    if !template.contains(PATH_MARKER) {
        args.push(path.to_string());
    }
    args
}

/// Sustituye los marcadores en una sola pasada: lo que se inserta (una ruta
/// que contenga `{line}`, por ejemplo) no se vuelve a mirar.
fn substitute(arg: &str, markers: &[(&str, &str)]) -> String {
    let mut result = String::with_capacity(arg.len());
    let mut rest = arg;
    'scan: while !rest.is_empty() {
        for (marker, value) in markers {
            if let Some(after) = rest.strip_prefix(marker) {
                result.push_str(value);
                rest = after;
                continue 'scan;
            }
        }
        let mut characters = rest.chars();
        if let Some(character) = characters.next() {
            result.push(character);
        }
        rest = characters.as_str();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_commands_are_unique() {
        let mut ids: Vec<&str> = EDITORS.iter().map(|editor| editor.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        // Ninguno choca con los valores especiales del ajuste.
        for special in ["auto", "system", "custom"] {
            assert!(editor_definition(special).is_none(), "{special}");
        }
    }

    #[test]
    fn editors_are_found_by_id_or_by_command() {
        assert_eq!(
            editor_definition("vscode").unwrap().name,
            "Visual Studio Code"
        );
        assert_eq!(editor_definition("code").unwrap().id, "vscode");
        assert_eq!(editor_definition("zeditor").unwrap().id, "zed");
        assert_eq!(editor_definition("subl").unwrap().id, "sublime");
        assert!(editor_definition("vim").is_none());
    }

    #[test]
    fn each_style_puts_the_line_where_its_editor_expects_it() {
        let path = "/home/ana/my repo/src/main.rs";
        assert_eq!(
            editor_arguments(LineStyle::Goto, path, Some(42)),
            ["--goto", "/home/ana/my repo/src/main.rs:42:1"]
        );
        assert_eq!(
            editor_arguments(LineStyle::Suffix, path, Some(42)),
            ["/home/ana/my repo/src/main.rs:42:1"]
        );
        assert_eq!(
            editor_arguments(LineStyle::JetBrains, path, Some(42)),
            ["--line", "42", "/home/ana/my repo/src/main.rs"]
        );
        assert_eq!(
            editor_arguments(LineStyle::NotepadPlusPlus, r"C:\repo\a b.txt", Some(7)),
            ["-n7", r"C:\repo\a b.txt"]
        );
        // Sin línea (o para una carpeta), solo la ruta, en todos.
        for style in [
            LineStyle::Goto,
            LineStyle::Suffix,
            LineStyle::JetBrains,
            LineStyle::NotepadPlusPlus,
        ] {
            assert_eq!(editor_arguments(style, path, None), [path]);
        }
    }

    #[test]
    fn templates_split_on_spaces_and_group_quotes() {
        assert_eq!(
            split_arguments(r#"  -w  --line {line} "{path}" 'a b'c "" "#),
            ["-w", "--line", "{line}", "{path}", "a bc", ""]
        );
        // La barra invertida es literal (rutas de Windows).
        assert_eq!(
            split_arguments(r#"/d C:\tools\ed "C:\Program Files\x""#),
            ["/d", r"C:\tools\ed", r"C:\Program Files\x"]
        );
        assert!(split_arguments("   ").is_empty());
    }

    #[test]
    fn custom_templates_substitute_markers_inside_each_argument() {
        let path = "/tmp/dir with spaces/ñandú.txt";
        assert_eq!(
            custom_arguments("--goto {path}:{line}:{column}", path, Some(12)),
            ["--goto", "/tmp/dir with spaces/ñandú.txt:12:1"]
        );
        // Sin línea, 1; sin `{path}`, la ruta al final.
        assert_eq!(
            custom_arguments("+{line} -c {column}", path, None),
            ["+1", "-c", "1", path]
        );
        assert_eq!(custom_arguments("", path, Some(3)), [path]);
        // Lo que trae la ruta no se vuelve a interpretar.
        assert_eq!(
            custom_arguments("{path}", "/tmp/{line} & $(rm -rf).txt", Some(9)),
            ["/tmp/{line} & $(rm -rf).txt"]
        );
    }
}
