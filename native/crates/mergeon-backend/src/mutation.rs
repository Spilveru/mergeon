//! Contrato de resultado de las mutaciones (port de
//! `electron/ipc/handler-utils.js` y de las piezas de `git-handlers.js` que
//! rodean a `registerRepoMutationHandler`).
//!
//! En la app Electron una mutación devuelve siempre un objeto estructurado y
//! nunca lanza. Ese objeto puede ser:
//!
//! - el resultado de la operación, tal cual, más `operationId`;
//! - ese mismo resultado con `success` / `operation` / `outcome` / `code` /
//!   `message` pisados, cuando la cola canceló la operación o venció su
//!   timeout pero la tarea devolvió un resultado que no lo refleja;
//! - un fallo fabricado por el handler: `IPC_ERROR` si la operación lanzó, o
//!   `CANCELLED` / `TIMEOUT` si la cola la rechazó.
//!
//! [`Mutation<T>`] conserva el resultado tipado de `mergeon-git-ops` y expone
//! los campos comunes con el mismo significado en los tres casos;
//! [`Mutation::to_json`] reconstruye el objeto exacto del original.

use std::time::Duration;

use mergeon_git_ops::undo::RefCapture;
use mergeon_git_ops::{
    MutationResult, OperationResult, Outcome, ResolveConflictResult, SequenceActionResult,
    SetIdentityResult,
};
use mergeon_git_query::{PathError, QueryError};
use mergeon_gitcmd::{ControlCode, GitError, LOCAL_TIMEOUT, NETWORK_TIMEOUT, StructuredStatus};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value, json};

/// Mutaciones que hablan con un remoto: su timeout es el de red.
pub const NETWORK_MUTATIONS: [&str; 4] = ["clone", "fetch", "pull", "push"];

pub(crate) const TIMEOUT_MESSAGE: &str = "Git operation timed out and was stopped.";
pub(crate) const CANCELLED_MESSAGE: &str = "Git operation was cancelled.";

/// Mensaje al usuario de una operación detenida por la cola.
pub(crate) fn control_message(code: ControlCode) -> &'static str {
    match code {
        ControlCode::Timeout => TIMEOUT_MESSAGE,
        ControlCode::Cancelled => CANCELLED_MESSAGE,
    }
}

/// `createOperationId`.
pub(crate) fn create_operation_id(operation: &str) -> String {
    format!("{operation}-{}", uuid::Uuid::new_v4())
}

/// `getMutationTimeout`: el timeout configurado en el handler o, si no lo
/// hay, el de red para las mutaciones de red y el local para el resto.
pub fn get_mutation_timeout(operation: &str, configured_timeout: Option<Duration>) -> Duration {
    configured_timeout
        .filter(|timeout| !timeout.is_zero())
        .unwrap_or(if NETWORK_MUTATIONS.contains(&operation) {
            NETWORK_TIMEOUT
        } else {
            LOCAL_TIMEOUT
        })
}

/// `String(value || '')`.
fn js_string_or_empty(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(false) => String::new(),
        Value::Bool(true) => "true".to_string(),
        Value::Number(number) => {
            if number.as_f64() == Some(0.0) {
                String::new()
            } else {
                number.to_string()
            }
        }
        Value::String(text) => text.clone(),
        // `String([a, b])` une con comas el `String()` de cada elemento.
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                Value::String(text) => text.clone(),
                Value::Bool(flag) => flag.to_string(),
                Value::Number(number) => number.to_string(),
                other => js_string_or_empty(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `fingerprintKeyValue`: FNV-1a de 32 bits sobre la primera unidad UTF-16 de
/// cada carácter, en hexadecimal. Sirve para que dos solicitudes con el mismo
/// secreto compartan clave sin que el secreto aparezca en ella.
pub fn fingerprint_key_value(value: &Value) -> String {
    let mut hash: u32 = 2_166_136_261;
    for character in js_string_or_empty(value).chars() {
        let mut units = [0u16; 2];
        let first_unit = character.encode_utf16(&mut units)[0];
        hash ^= u32::from(first_unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("{hash:x}")
}

/// `/(token|password|secret|authorization)/i.test(key)`.
fn is_secret_key(key: &str) -> bool {
    let key = key.to_lowercase();
    ["token", "password", "secret", "authorization"]
        .iter()
        .any(|word| key.contains(word))
}

/// `normalizeMutationKeyValue`: claves de objeto ordenadas y secretos
/// sustituidos por su huella.
pub fn normalize_mutation_key_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(normalize_mutation_key_value).collect())
        }
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            let mut result = Map::new();
            for key in keys {
                let entry = &object[key];
                let normalized = if is_secret_key(key) {
                    Value::String(format!("[fingerprint:{}]", fingerprint_key_value(entry)))
                } else {
                    normalize_mutation_key_value(entry)
                };
                result.insert(key.clone(), normalized);
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}

/// `buildMutationRequestKey`: clave de dedupe de una solicitud. `args` es el
/// array de argumentos del canal.
pub fn build_mutation_request_key(operation: &str, args: &Value) -> String {
    format!("{operation}:{}", normalize_mutation_key_value(args))
}

/// `code` de un resultado fabricado por el handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureCode {
    /// La operación lanzó en vez de devolver un resultado.
    IpcError,
    Cancelled,
    Timeout,
}

impl FailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureCode::IpcError => "IPC_ERROR",
            FailureCode::Cancelled => "CANCELLED",
            FailureCode::Timeout => "TIMEOUT",
        }
    }

    pub fn control_code(self) -> Option<ControlCode> {
        match self {
            FailureCode::IpcError => None,
            FailureCode::Cancelled => Some(ControlCode::Cancelled),
            FailureCode::Timeout => Some(ControlCode::Timeout),
        }
    }
}

impl From<ControlCode> for FailureCode {
    fn from(code: ControlCode) -> Self {
        match code {
            ControlCode::Cancelled => FailureCode::Cancelled,
            ControlCode::Timeout => FailureCode::Timeout,
        }
    }
}

/// Resultado que fabrica el handler cuando la operación no devolvió el suyo
/// (`createStructuredHandlerError` / `createGenericOperationFailure` /
/// `createControlFailure`).
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerFailure {
    pub operation: String,
    /// `failed`, `cancelled` o `timeout`.
    pub outcome: Outcome,
    pub code: FailureCode,
    pub message: String,
    /// El resto del objeto: los campos nulos del fallo genérico y el `extra`
    /// del canal (`branch`, `filePath`, `filesAffected`...).
    pub fields: Map<String, Value>,
}

impl HandlerFailure {
    /// Campo adicional del objeto, por su nombre en el original.
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    fn to_object(&self) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("success".into(), Value::Bool(false));
        object.insert("operation".into(), Value::String(self.operation.clone()));
        object.insert("outcome".into(), json!(self.outcome.as_str()));
        object.insert("code".into(), json!(self.code.as_str()));
        object.insert("message".into(), Value::String(self.message.clone()));
        for (key, value) in &self.fields {
            object.insert(key.clone(), value.clone());
        }
        object
    }
}

/// `toErrorMessage(error, fallback)`.
fn to_error_message<'a>(message: &'a str, fallback: &'a str) -> &'a str {
    if message.is_empty() {
        fallback
    } else {
        message
    }
}

/// `createStructuredHandlerError`.
pub fn create_structured_handler_error(
    operation: &str,
    message_prefix: &str,
    error_message: &str,
    extra: Map<String, Value>,
) -> HandlerFailure {
    let fallback = format!("{operation} failed");
    HandlerFailure {
        operation: operation.to_string(),
        outcome: Outcome::Failed,
        code: FailureCode::IpcError,
        message: format!(
            "{message_prefix}: {}",
            to_error_message(error_message, &fallback)
        ),
        fields: extra,
    }
}

/// `createGenericOperationFailure`: como [`create_structured_handler_error`],
/// con todos los campos del resultado de una mutación presentes y nulos.
pub fn create_generic_operation_failure(
    operation: &str,
    message_prefix: &str,
    error_message: &str,
    extra: Map<String, Value>,
) -> HandlerFailure {
    let mut fields = Map::new();
    for key in [
        "status",
        "operationState",
        "conflictState",
        "branch",
        "remote",
        "tag",
        "stashRef",
        "commit",
        "mode",
        "filePath",
    ] {
        fields.insert(key.to_string(), Value::Null);
    }
    fields.insert("filesAffected".to_string(), json!([]));
    fields.extend(extra);
    create_structured_handler_error(operation, message_prefix, error_message, fields)
}

/// Qué constructor usa el handler para un fallo que no es de control
/// (`config.failure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FailureShape {
    /// `createGenericOperationFailure`.
    #[default]
    Generic,
    /// `createStructuredHandlerError`.
    Structured,
}

/// La rama de control de `createControlFailure`: siempre el fallo genérico,
/// con `outcome`, `code` y `message` de la cancelación o el timeout.
pub(crate) fn create_control_failure(
    operation: &str,
    code: ControlCode,
    extra: Map<String, Value>,
) -> HandlerFailure {
    let mut failure = create_generic_operation_failure(operation, "", "", extra);
    failure.outcome = code.into();
    failure.code = code.into();
    failure.message = control_message(code).to_string();
    failure
}

/// Lo que el handler lee de un resultado estructurado sin conocer su tipo.
pub trait StructuredResult {
    fn success(&self) -> bool;
    /// `operation`; `None` si el resultado no lo lleva.
    fn operation(&self) -> Option<&str>;
    /// `outcome`; `None` si el resultado no lo lleva.
    fn outcome(&self) -> Option<Outcome>;
    fn code(&self) -> &str;
    fn message(&self) -> &str;
    /// Estado del repositorio tras la operación, si el resultado lo trae.
    fn status(&self) -> Option<&StructuredStatus> {
        None
    }
}

impl StructuredResult for MutationResult {
    fn success(&self) -> bool {
        self.success
    }
    fn operation(&self) -> Option<&str> {
        Some(self.operation.as_str())
    }
    fn outcome(&self) -> Option<Outcome> {
        Some(self.outcome)
    }
    fn code(&self) -> &str {
        self.code.as_str()
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn status(&self) -> Option<&StructuredStatus> {
        self.status.as_ref()
    }
}

impl StructuredResult for OperationResult {
    fn success(&self) -> bool {
        self.success
    }
    fn operation(&self) -> Option<&str> {
        Some(self.operation.as_str())
    }
    fn outcome(&self) -> Option<Outcome> {
        Some(self.outcome)
    }
    fn code(&self) -> &str {
        self.code.as_str()
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn status(&self) -> Option<&StructuredStatus> {
        self.status.as_ref()
    }
}

impl StructuredResult for ResolveConflictResult {
    fn success(&self) -> bool {
        self.success
    }
    fn operation(&self) -> Option<&str> {
        None
    }
    fn outcome(&self) -> Option<Outcome> {
        None
    }
    fn code(&self) -> &str {
        self.code.as_str()
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn status(&self) -> Option<&StructuredStatus> {
        self.status.as_ref()
    }
}

impl StructuredResult for SequenceActionResult {
    fn success(&self) -> bool {
        self.success
    }
    fn operation(&self) -> Option<&str> {
        self.operation.map(|operation| operation.as_str())
    }
    fn outcome(&self) -> Option<Outcome> {
        Some(self.outcome)
    }
    fn code(&self) -> &str {
        self.code.as_str()
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn status(&self) -> Option<&StructuredStatus> {
        self.status.as_ref()
    }
}

impl StructuredResult for SetIdentityResult {
    fn success(&self) -> bool {
        self.success
    }
    fn operation(&self) -> Option<&str> {
        Some(self.operation.as_str())
    }
    fn outcome(&self) -> Option<Outcome> {
        Some(self.outcome)
    }
    fn code(&self) -> &str {
        self.code.as_str()
    }
    fn message(&self) -> &str {
        &self.message
    }
}

/// De dónde salió el resultado de una mutación.
#[derive(Debug, Clone, PartialEq)]
pub enum MutationBody<T> {
    /// La operación corrió y este es su resultado, tal cual.
    Completed(T),
    /// La operación devolvió un resultado, pero la cola la había cancelado o
    /// había vencido su timeout: se conservan sus campos y se pisan `success`,
    /// `operation`, `outcome`, `code` y `message`.
    Controlled {
        result: T,
        /// Nombre de la operación del handler (`config.operation`).
        operation: String,
        code: ControlCode,
    },
    /// No hubo resultado de la operación: lo fabricó el handler.
    Failed(HandlerFailure),
}

/// Respuesta de un canal de mutación: nunca es un error.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation<T> {
    /// `operationId`: el de la operación en la cola, que también llevan sus
    /// eventos. `None` solo en un fallo `IPC_ERROR`, como en el original.
    pub operation_id: Option<String>,
    pub body: MutationBody<T>,
    /// HEAD, refs y copia de un descarte leídos antes y después de la
    /// operación, si su canal lo pide (lo que hace falta para deshacerla).
    /// No viaja en [`Mutation::to_json`].
    pub capture: Option<RefCapture>,
}

impl<T> Mutation<T> {
    pub(crate) fn failed(failure: HandlerFailure, operation_id: Option<String>) -> Self {
        Self {
            operation_id,
            body: MutationBody::Failed(failure),
            capture: None,
        }
    }

    pub fn operation_id(&self) -> Option<&str> {
        self.operation_id.as_deref()
    }

    /// Lo leído antes y después de la operación ([`Mutation::capture`]).
    pub fn capture(&self) -> Option<&RefCapture> {
        self.capture.as_ref()
    }

    /// El resultado tipado de la operación, si llegó a devolver uno. Cuando
    /// la cola pisó el resultado ([`MutationBody::Controlled`]) sus campos
    /// `success` / `outcome` / `code` / `message` ya no son los vigentes: hay
    /// que leerlos de [`Mutation`].
    pub fn result(&self) -> Option<&T> {
        match &self.body {
            MutationBody::Completed(result) | MutationBody::Controlled { result, .. } => {
                Some(result)
            }
            MutationBody::Failed(_) => None,
        }
    }

    pub fn into_result(self) -> Option<T> {
        match self.body {
            MutationBody::Completed(result) | MutationBody::Controlled { result, .. } => {
                Some(result)
            }
            MutationBody::Failed(_) => None,
        }
    }

    /// El fallo fabricado por el handler, si lo hubo.
    pub fn failure(&self) -> Option<&HandlerFailure> {
        match &self.body {
            MutationBody::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    /// `CANCELLED` / `TIMEOUT` cuando fue la cola quien detuvo la operación.
    pub fn handler_control_code(&self) -> Option<ControlCode> {
        match &self.body {
            MutationBody::Completed(_) => None,
            MutationBody::Controlled { code, .. } => Some(*code),
            MutationBody::Failed(failure) => failure.code.control_code(),
        }
    }
}

impl<T: StructuredResult> Mutation<T> {
    pub fn success(&self) -> bool {
        match &self.body {
            MutationBody::Completed(result) => result.success(),
            MutationBody::Controlled { .. } | MutationBody::Failed(_) => false,
        }
    }

    pub fn operation(&self) -> Option<&str> {
        match &self.body {
            MutationBody::Completed(result) => result.operation(),
            MutationBody::Controlled { operation, .. } => Some(operation),
            MutationBody::Failed(failure) => Some(&failure.operation),
        }
    }

    pub fn outcome(&self) -> Option<Outcome> {
        match &self.body {
            MutationBody::Completed(result) => result.outcome(),
            MutationBody::Controlled { code, .. } => Some((*code).into()),
            MutationBody::Failed(failure) => Some(failure.outcome),
        }
    }

    pub fn code(&self) -> &str {
        match &self.body {
            MutationBody::Completed(result) => result.code(),
            MutationBody::Controlled { code, .. } => code.as_str(),
            MutationBody::Failed(failure) => failure.code.as_str(),
        }
    }

    pub fn message(&self) -> &str {
        match &self.body {
            MutationBody::Completed(result) => result.message(),
            MutationBody::Controlled { code, .. } => control_message(*code),
            MutationBody::Failed(failure) => &failure.message,
        }
    }

    /// Estado del repositorio que acompaña al resultado, si lo hay.
    pub fn status(&self) -> Option<&StructuredStatus> {
        self.result().and_then(StructuredResult::status)
    }

    /// La operación terminó por cancelación (de la cola o de la propia operación).
    pub fn is_cancelled(&self) -> bool {
        self.code() == ControlCode::Cancelled.as_str()
    }

    /// La operación terminó por timeout.
    pub fn is_timeout(&self) -> bool {
        self.code() == ControlCode::Timeout.as_str()
    }
}

impl<T: Serialize> Mutation<T> {
    /// El objeto que el handler devolvía al renderer, campo a campo.
    pub fn to_json(&self) -> Value {
        let mut object = match &self.body {
            MutationBody::Completed(result) => to_object(result),
            MutationBody::Controlled {
                result,
                operation,
                code,
            } => {
                let mut object = to_object(result);
                object.insert("success".into(), Value::Bool(false));
                object.insert("operation".into(), Value::String(operation.clone()));
                object.insert("outcome".into(), json!(Outcome::from(*code).as_str()));
                object.insert("code".into(), json!(code.as_str()));
                object.insert("message".into(), json!(control_message(*code)));
                object
            }
            MutationBody::Failed(failure) => {
                let mut object = failure.to_object();
                // El fallo de control lleva siempre `operationId` (aunque sea
                // nulo); el `IPC_ERROR` no lo lleva nunca.
                if failure.code.control_code().is_some() {
                    object.insert("operationId".into(), json!(self.operation_id));
                }
                return Value::Object(object);
            }
        };
        if let Some(operation_id) = &self.operation_id {
            object.insert("operationId".into(), Value::String(operation_id.clone()));
        }
        Value::Object(object)
    }
}

fn to_object<T: Serialize>(result: &T) -> Map<String, Value> {
    match serde_json::to_value(result) {
        Ok(Value::Object(object)) => object,
        _ => Map::new(),
    }
}

impl<T: Serialize> Serialize for Mutation<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

/// Lo que en el original es una excepción dentro de un handler de mutación:
/// la operación no devolvió un resultado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Thrown {
    /// `error.message`.
    pub message: String,
    /// `error.code` (p. ej. `EACCES`), que viaja en el evento de la operación.
    pub code: Option<String>,
    /// Cancelación o timeout detrás del error.
    pub control: Option<ControlCode>,
    /// Operación de la cola que lanzó (`error.operationId`).
    pub operation_id: Option<String>,
}

impl Thrown {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            control: None,
            operation_id: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
}

impl std::fmt::Display for Thrown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<GitError> for Thrown {
    fn from(error: GitError) -> Self {
        Self {
            message: error.message,
            code: None,
            control: error.control,
            operation_id: None,
        }
    }
}

impl From<PathError> for Thrown {
    fn from(error: PathError) -> Self {
        Self::new(error.message())
    }
}

impl From<QueryError> for Thrown {
    fn from(error: QueryError) -> Self {
        Self::new(error.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_like_the_original_fnv1a() {
        // Valores contrastados con `fingerprintKeyValue` en Node.
        assert_eq!(fingerprint_key_value(&json!("")), "811c9dc5");
        assert_eq!(fingerprint_key_value(&Value::Null), "811c9dc5");
        assert_eq!(fingerprint_key_value(&json!("a")), "e40c292c");
        assert_eq!(fingerprint_key_value(&json!("abc")), "1a47e90b");
        assert_eq!(fingerprint_key_value(&json!("gho_secret")), "c934bfcc");
        // Un carácter fuera del BMP solo aporta su primera unidad UTF-16.
        assert_eq!(fingerprint_key_value(&json!("😀")), "37822568");
        // `String(value || '')` de lo que no es texto.
        assert_eq!(fingerprint_key_value(&json!(42)), "87e38583");
        assert_eq!(fingerprint_key_value(&json!(0)), "811c9dc5");
        assert_eq!(fingerprint_key_value(&json!(true)), "4db211e5");
        assert_eq!(fingerprint_key_value(&json!(false)), "811c9dc5");
        // Sin ceros a la izquierda, como `toString(16)`.
        assert_eq!(fingerprint_key_value(&json!(["a", "b"])), "8876b46");
        assert_eq!(fingerprint_key_value(&json!({ "a": 1 })), "fa5f6c9f");
    }

    #[test]
    fn builds_request_keys_with_sorted_keys_and_fingerprinted_secrets() {
        let key = build_mutation_request_key(
            "fetch",
            &json!([{ "remote": "origin", "authContext": { "token": "abc", "installationId": 7 }, "allRemotes": false }]),
        );
        assert_eq!(
            key,
            r#"fetch:[{"allRemotes":false,"authContext":{"installationId":7,"token":"[fingerprint:1a47e90b]"},"remote":"origin"}]"#
        );

        // Cualquier clave con pinta de secreto se sustituye entera, sea cual
        // sea su valor.
        let key = build_mutation_request_key(
            "push",
            &json!(["/repo", { "authToken": "abc", "Password": null, "nested": [{ "clientSecret": "abc" }] }]),
        );
        assert_eq!(
            key,
            r#"push:["/repo",{"Password":"[fingerprint:811c9dc5]","authToken":"[fingerprint:1a47e90b]","nested":[{"clientSecret":"[fingerprint:1a47e90b]"}]}]"#
        );
        assert!(!key.contains("abc\""));
    }

    #[test]
    fn identical_requests_share_a_key_and_different_ones_do_not() {
        let first = build_mutation_request_key("stage", &json!(["/repo", "a.txt"]));
        assert_eq!(first, r#"stage:["/repo","a.txt"]"#);
        assert_eq!(
            first,
            build_mutation_request_key("stage", &json!(["/repo", "a.txt"]))
        );
        assert_ne!(
            first,
            build_mutation_request_key("stage", &json!(["/repo", "b.txt"]))
        );
        assert_ne!(
            build_mutation_request_key("fetch", &json!([{ "authToken": "one" }])),
            build_mutation_request_key("fetch", &json!([{ "authToken": "two" }]))
        );
    }

    #[test]
    fn network_mutations_get_the_network_timeout() {
        for operation in NETWORK_MUTATIONS {
            assert_eq!(get_mutation_timeout(operation, None), NETWORK_TIMEOUT);
        }
        assert_eq!(get_mutation_timeout("stage", None), LOCAL_TIMEOUT);
        assert_eq!(get_mutation_timeout("push-tag", None), LOCAL_TIMEOUT);
        assert_eq!(
            get_mutation_timeout("push-tag", Some(NETWORK_TIMEOUT)),
            NETWORK_TIMEOUT
        );
        assert_eq!(
            get_mutation_timeout("fetch", Some(Duration::from_millis(5))),
            Duration::from_millis(5)
        );
        // `configuredTimeout || ...`: un 0 configurado cuenta como ausente.
        assert_eq!(
            get_mutation_timeout("stage", Some(Duration::ZERO)),
            LOCAL_TIMEOUT
        );
    }

    #[test]
    fn operation_ids_are_prefixed_and_unique() {
        let first = create_operation_id("stage");
        let second = create_operation_id("stage");
        assert!(first.starts_with("stage-"));
        assert_eq!(first.len(), "stage-".len() + 36);
        assert_ne!(first, second);
    }

    #[test]
    fn generic_failures_carry_every_mutation_field() {
        let mut extra = Map::new();
        extra.insert("filePath".into(), json!("src/app.js"));
        extra.insert("filesAffected".into(), json!(["src/app.js"]));
        let failure = create_generic_operation_failure(
            "stage",
            "Failed to stage file",
            "index locked",
            extra,
        );
        let mutation: Mutation<MutationResult> = Mutation::failed(failure, None);

        assert_eq!(
            mutation.to_json(),
            json!({
                "success": false,
                "operation": "stage",
                "outcome": "failed",
                "code": "IPC_ERROR",
                "message": "Failed to stage file: index locked",
                "status": null,
                "operationState": null,
                "conflictState": null,
                "branch": null,
                "remote": null,
                "tag": null,
                "stashRef": null,
                "commit": null,
                "mode": null,
                "filePath": "src/app.js",
                "filesAffected": ["src/app.js"]
            })
        );
        assert!(!mutation.success());
        assert_eq!(mutation.code(), "IPC_ERROR");
        assert_eq!(mutation.outcome(), Some(Outcome::Failed));
        assert_eq!(mutation.operation(), Some("stage"));
        assert_eq!(mutation.result(), None);
    }

    #[test]
    fn structured_failures_only_carry_their_extra() {
        let failure = create_structured_handler_error("push", "Failed to push", "", Map::new());
        assert_eq!(failure.message, "Failed to push: push failed");
        assert!(failure.fields.is_empty());
    }

    #[test]
    fn control_failures_use_the_generic_shape_and_always_carry_the_operation_id() {
        let mut extra = Map::new();
        extra.insert("branch".into(), json!("main"));
        let failure = create_control_failure("checkout", ControlCode::Timeout, extra);
        let mutation: Mutation<MutationResult> =
            Mutation::failed(failure.clone(), Some("checkout-1".into()));
        let json = mutation.to_json();

        assert_eq!(json["outcome"], "timeout");
        assert_eq!(json["code"], "TIMEOUT");
        assert_eq!(json["message"], "Git operation timed out and was stopped.");
        assert_eq!(json["branch"], "main");
        assert_eq!(json["filesAffected"], json!([]));
        assert_eq!(json["operationId"], "checkout-1");
        assert!(mutation.is_timeout());
        assert_eq!(mutation.handler_control_code(), Some(ControlCode::Timeout));

        let without_id: Mutation<MutationResult> = Mutation::failed(failure, None);
        assert_eq!(without_id.to_json()["operationId"], Value::Null);
        assert!(
            without_id
                .to_json()
                .as_object()
                .unwrap()
                .contains_key("operationId")
        );
    }

    #[test]
    fn a_controlled_result_keeps_its_fields_and_overrides_the_verdict() {
        let result = SetIdentityResult {
            success: true,
            operation: mergeon_git_ops::Operation::SetIdentity,
            outcome: Outcome::Completed,
            code: mergeon_git_ops::Code::Ok,
            message: "done".into(),
        };
        let mutation = Mutation {
            capture: None,
            operation_id: Some("op-1".into()),
            body: MutationBody::Controlled {
                result: result.clone(),
                operation: "custom".into(),
                code: ControlCode::Cancelled,
            },
        };

        assert!(!mutation.success());
        assert_eq!(mutation.operation(), Some("custom"));
        assert_eq!(mutation.outcome(), Some(Outcome::Cancelled));
        assert_eq!(mutation.code(), "CANCELLED");
        assert_eq!(mutation.message(), "Git operation was cancelled.");
        assert!(mutation.is_cancelled());
        assert_eq!(mutation.result(), Some(&result));
        assert_eq!(
            mutation.to_json(),
            json!({
                "success": false,
                "operation": "custom",
                "outcome": "cancelled",
                "code": "CANCELLED",
                "message": "Git operation was cancelled.",
                "operationId": "op-1"
            })
        );

        let completed = Mutation {
            capture: None,
            operation_id: Some("op-2".into()),
            body: MutationBody::Completed(result),
        };
        assert_eq!(
            serde_json::to_value(&completed).unwrap(),
            json!({
                "success": true,
                "operation": "set-identity",
                "outcome": "completed",
                "code": "OK",
                "message": "done",
                "operationId": "op-2"
            })
        );
    }
}
