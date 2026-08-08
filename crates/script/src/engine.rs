//! QuickJS-backed lifecycle hook execution.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use rquickjs::{
    CatchResultExt, Context, Ctx, Function, Module, Object, Persistent, Runtime, Value,
    function::Args,
};
use rusting_core::{RequestModel, ScriptRef, Variables};
use rusting_http::Response;

use crate::{
    api::{self, InvocationOutput},
    convert,
    types::{HOOK_BUDGET_MS, HookOutcome, HookStatus, ScriptError},
};

/// A session-scoped JavaScript hook engine.
///
/// Hook files are ES modules. Their default exports are named after the YAML
/// keys, and an explicit `path.js:functionName` reference selects another named
/// export. The script-facing contract is:
///
/// ```js
/// export function setup(rusting) {
///   rusting.setVariable("token", "abc");
///   console.log(rusting.variables.BASE_URL);
/// }
/// export function on_request(request, rusting) {
///   request.headers.push({ name: "X-Custom", value: "1" });
///   request.auth = { type: "basic", basic: { username: "u", password: "p" } };
///   request.body = { content: JSON.stringify({ a: 1 }), content_type: "application/json" };
///   rusting.notify("sending", "warning");
/// }
/// export function on_response(response, rusting) {
///   console.log(response.status, response.headers["content-type"]);
///   rusting.setVariable("id", JSON.parse(response.body).id);
/// }
/// ```
///
/// `rusting.variables` and the response object are read-only. API calls append
/// effects to the returned [`HookOutcome`]; they never mutate the environment.
/// Console methods append space-separated log lines. A hook receives only as
/// many leading arguments as its declared `Function.length` requests.
pub struct Engine {
    collection_root: PathBuf,
    cache: HashMap<PathBuf, Persistent<Object<'static>>>,
    module_generation: u64,
    context: Context,
    runtime: Runtime,
}

impl Engine {
    /// Create an engine rooted at a canonical collection directory.
    pub fn new(collection_root: PathBuf) -> Result<Self, ScriptError> {
        let canonical_root = canonicalize(&collection_root)?;
        let metadata =
            fs::metadata(&canonical_root).map_err(|error| io_error(&canonical_root, error))?;
        if !metadata.is_dir() {
            return Err(ScriptError::Load {
                path: canonical_root,
                message: "collection root is not a directory".into(),
            });
        }

        let runtime = Runtime::new().map_err(|error| ScriptError::Load {
            path: canonical_root.clone(),
            message: error.to_string(),
        })?;
        let context = Context::full(&runtime).map_err(|error| ScriptError::Load {
            path: canonical_root.clone(),
            message: error.to_string(),
        })?;

        Ok(Self {
            collection_root: canonical_root,
            cache: HashMap::new(),
            module_generation: 0,
            context,
            runtime,
        })
    }

    /// Discard the cached evaluation for one script path.
    pub fn invalidate(&mut self, path: &Path) {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.collection_root.join(path)
        };
        let key = candidate.canonicalize().unwrap_or(candidate);
        self.cache.remove(&key);
    }

    /// Discard every cached module evaluation.
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
    }

    pub fn run_setup(&mut self, script: &ScriptRef, variables: &Variables) -> HookOutcome {
        self.run_hook(script, HookInput::Setup, variables).0
    }

    /// Run a pre-request hook and atomically replace `request` after a valid,
    /// complete conversion back from JavaScript.
    pub fn run_on_request(
        &mut self,
        script: &ScriptRef,
        request: &mut RequestModel,
        variables: &Variables,
    ) -> HookOutcome {
        let (outcome, converted) = self.run_hook(script, HookInput::Request(request), variables);
        if matches!(outcome.status, HookStatus::Success)
            && let Some(converted) = converted
        {
            *request = converted;
        }
        outcome
    }

    pub fn run_on_response(
        &mut self,
        script: &ScriptRef,
        response: &Response,
        request: &RequestModel,
        variables: &Variables,
    ) -> HookOutcome {
        self.run_hook(script, HookInput::Response { response, request }, variables)
            .0
    }

    fn run_hook(
        &mut self,
        script: &ScriptRef,
        input: HookInput<'_>,
        variables: &Variables,
    ) -> (HookOutcome, Option<RequestModel>) {
        let path = match self.resolve_script(&script.path) {
            Ok(path) => path,
            Err(error) => return (error_outcome(error, InvocationOutput::default()), None),
        };

        let output = InvocationOutput::default();
        let started = Instant::now();
        let interrupted = Arc::new(AtomicBool::new(false));
        let interrupted_for_handler = interrupted.clone();
        self.runtime.set_interrupt_handler(Some(Box::new(move || {
            if started.elapsed().as_millis() >= u128::from(HOOK_BUDGET_MS) {
                interrupted_for_handler.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        })));

        let context = self.context.clone();
        let result = context
            .with(|ctx| self.invoke_in_context(&ctx, &path, script, input, variables, &output));
        self.runtime.set_interrupt_handler(None);

        let result = if interrupted.load(Ordering::Relaxed) {
            Err(ScriptError::TimedOut(HOOK_BUDGET_MS))
        } else {
            result
        };

        match result {
            Ok(converted) => (
                HookOutcome {
                    status: HookStatus::Success,
                    logs: output.logs(),
                    effects: output.effects(),
                },
                converted,
            ),
            Err(error) => (error_outcome(error, output), None),
        }
    }

    fn invoke_in_context<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &Path,
        script: &ScriptRef,
        input: HookInput<'_>,
        variables: &Variables,
        output: &InvocationOutput,
    ) -> Result<Option<RequestModel>, ScriptError> {
        let (request_value, response_value, original_request) = match input {
            HookInput::Setup => (None, None, None),
            HookInput::Request(request) => (
                Some(json_to_js(
                    ctx,
                    convert::request_to_value(request),
                    &script.function,
                )?),
                None,
                Some(request),
            ),
            HookInput::Response { response, request } => {
                let response_value =
                    json_to_js(ctx, convert::response_to_value(response), &script.function)?;
                api::freeze_response(ctx, response_value.clone())
                    .map_err(|error| threw(ctx, &script.function, error))?;
                (
                    Some(json_to_js(
                        ctx,
                        convert::request_to_value(request),
                        &script.function,
                    )?),
                    Some(response_value),
                    None,
                )
            }
        };

        let rusting = api::install(
            ctx,
            variables,
            request_value.clone(),
            response_value.clone(),
            output,
        )
        .map_err(|error| threw(ctx, &script.function, error))?;

        let namespace = self.module_namespace(ctx, path)?;
        let exported: Value = namespace
            .get(script.function.as_str())
            .map_err(|error| threw(ctx, &script.function, error))?;
        let function = exported
            .into_function()
            .ok_or_else(|| ScriptError::MissingFunction {
                path: path.to_path_buf(),
                function: script.function.clone(),
            })?;

        let mut arguments = Vec::with_capacity(2);
        match input {
            HookInput::Setup => arguments.push(rusting.into_value()),
            HookInput::Request(_) => {
                let value = request_value.clone().ok_or_else(|| {
                    internal_hook_error(&script.function, "request value was not prepared")
                })?;
                arguments.push(value);
                arguments.push(rusting.into_value());
            }
            HookInput::Response { .. } => {
                let value = response_value.ok_or_else(|| {
                    internal_hook_error(&script.function, "response value was not prepared")
                })?;
                arguments.push(value);
                arguments.push(rusting.into_value());
            }
        }
        call_adapted(ctx, &function, &arguments, &script.function)?;

        let Some(original_request) = original_request else {
            return Ok(None);
        };
        let request_value = request_value.ok_or_else(|| {
            internal_hook_error(&script.function, "request value was not prepared")
        })?;
        let json = ctx
            .json_stringify(request_value)
            .map_err(|error| threw(ctx, &script.function, error))?
            .ok_or_else(|| ScriptError::Threw {
                function: script.function.clone(),
                message: "request object cannot be represented as JSON".into(),
            })?
            .to_string()
            .map_err(|error| threw(ctx, &script.function, error))?;
        let value = serde_json::from_str(&json).map_err(|error| ScriptError::Threw {
            function: script.function.clone(),
            message: format!("invalid request object: {error}"),
        })?;
        let converted = convert::request_from_value(value, original_request).map_err(|error| {
            ScriptError::Threw {
                function: script.function.clone(),
                message: format!("invalid request object: {error}"),
            }
        })?;
        Ok(Some(converted))
    }

    fn module_namespace<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &Path,
    ) -> Result<Object<'js>, ScriptError> {
        if let Some(cached) = self.cache.get(path) {
            return cached
                .clone()
                .restore(ctx)
                .map_err(|error| ScriptError::Load {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
        }

        let source = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
        self.module_generation = self.module_generation.wrapping_add(1);
        let module_name = format!("{}?rusting={}", path.display(), self.module_generation);
        let module = Module::declare(ctx.clone(), module_name, source)
            .catch(ctx)
            .map_err(|error| ScriptError::Load {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let (module, promise) = module
            .eval()
            .catch(ctx)
            .map_err(|error| ScriptError::Load {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        promise
            .finish::<()>()
            .catch(ctx)
            .map_err(|error| ScriptError::Load {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let namespace = module
            .namespace()
            .catch(ctx)
            .map_err(|error| ScriptError::Load {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        self.cache
            .insert(path.to_path_buf(), Persistent::save(ctx, namespace.clone()));
        Ok(namespace)
    }

    fn resolve_script(&self, requested: &Path) -> Result<PathBuf, ScriptError> {
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.collection_root.join(requested)
        };
        let canonical = canonicalize(&candidate)?;
        if !canonical.starts_with(&self.collection_root) {
            return Err(ScriptError::OutsideCollection(canonical));
        }
        if !canonical.is_file() {
            return Err(ScriptError::NotFound(canonical));
        }
        Ok(canonical)
    }
}

#[derive(Clone, Copy)]
enum HookInput<'a> {
    Setup,
    Request(&'a RequestModel),
    Response {
        response: &'a Response,
        request: &'a RequestModel,
    },
}

fn call_adapted<'js>(
    ctx: &Ctx<'js>,
    function: &Function<'js>,
    arguments: &[Value<'js>],
    function_name: &str,
) -> Result<(), ScriptError> {
    let declared: usize = function
        .get("length")
        .map_err(|error| threw(ctx, function_name, error))?;
    let count = declared.min(arguments.len());
    let mut args = Args::new(ctx.clone(), count);
    for argument in arguments.iter().take(count) {
        args.push_arg(argument.clone())
            .map_err(|error| threw(ctx, function_name, error))?;
    }
    let _: Value = args
        .apply(function)
        .catch(ctx)
        .map_err(|error| ScriptError::Threw {
            function: function_name.to_owned(),
            message: error.to_string(),
        })?;
    Ok(())
}

fn json_to_js<'js>(
    ctx: &Ctx<'js>,
    value: serde_json::Value,
    function: &str,
) -> Result<Value<'js>, ScriptError> {
    let json = serde_json::to_string(&value).map_err(|error| ScriptError::Threw {
        function: function.to_owned(),
        message: error.to_string(),
    })?;
    ctx.json_parse(json)
        .map_err(|error| threw(ctx, function, error))
}

fn threw(ctx: &Ctx<'_>, function: &str, error: rquickjs::Error) -> ScriptError {
    ScriptError::Threw {
        function: function.to_owned(),
        message: rquickjs::CaughtError::from_error(ctx, error).to_string(),
    }
}

fn internal_hook_error(function: &str, message: &str) -> ScriptError {
    ScriptError::Threw {
        function: function.to_owned(),
        message: message.to_owned(),
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, ScriptError> {
    path.canonicalize().map_err(|error| io_error(path, error))
}

fn io_error(path: &Path, error: std::io::Error) -> ScriptError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ScriptError::NotFound(path.to_path_buf())
    } else {
        ScriptError::Load {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

fn error_outcome(error: ScriptError, output: InvocationOutput) -> HookOutcome {
    HookOutcome {
        status: HookStatus::Error(error.to_string()),
        logs: output.logs(),
        effects: output.effects(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use rusting_core::{AuthKind, HttpMethod, KeyValue};
    use rusting_http::{SentRequest, Timings};

    use crate::types::{Effect, Severity, Stream};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("rusting-script-{}-{sequence}", std::process::id()));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn write(&self, name: &str, source: &str) -> ScriptRef {
            fs::write(self.0.join(name), source).expect("write script");
            ScriptRef {
                path: PathBuf::from(name),
                function: "setup".into(),
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn response() -> Response {
        let mut timings = Timings::default();
        timings.total = Some(Duration::from_millis(12));
        Response {
            status: 200,
            reason: "OK".into(),
            url: "https://example.test".into(),
            headers: vec![KeyValue::new("Content-Type", "application/json")],
            cookies: Vec::new(),
            body: br#"{"id":7}"#.to_vec(),
            timings,
            sent: SentRequest::default(),
        }
    }

    #[test]
    fn executes_all_hooks_effects_mutation_and_console_contract() {
        let directory = TestDirectory::new();
        directory.write(
            "hooks.js",
            r#"
                export function setup(rusting) {
                    rusting.setVariable("token", "abc");
                    rusting.clearVariable("old");
                    rusting.clearAllVariables();
                    console.log(rusting.variables.BASE_URL, 2);
                    console.warn("warning output");
                }
                export function on_request(request, rusting) {
                    request.method = "POST";
                    request.headers.push({ name: "X-Custom", value: "1" });
                    request.auth = { type: "basic", basic: { username: "u", password: "p" } };
                    request.body = { content: JSON.stringify({ a: 1 }), content_type: "application/json" };
                    rusting.notify("sending", "warning");
                }
                export function on_response(response, rusting) {
                    console.info(response.status, response.headers["content-type"]);
                    rusting.setVariable("id", JSON.parse(response.body).id);
                }
            "#,
        );
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let variables = Variables::from([("BASE_URL".into(), "https://example.test".into())]);

        let setup = engine.run_setup(
            &ScriptRef {
                path: "hooks.js".into(),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(matches!(setup.status, HookStatus::Success));
        assert_eq!(setup.logs[0].stream, Stream::Out);
        assert_eq!(setup.logs[0].text, "https://example.test 2");
        assert_eq!(
            setup.logs[1],
            crate::types::LogLine {
                stream: Stream::Err,
                text: "warning output".into()
            }
        );
        assert_eq!(
            setup.effects,
            vec![
                Effect::SetVariable {
                    name: "token".into(),
                    value: "abc".into()
                },
                Effect::ClearVariable { name: "old".into() },
                Effect::ClearAllVariables,
            ]
        );

        let mut request = RequestModel::default();
        let request_outcome = engine.run_on_request(
            &ScriptRef {
                path: "hooks.js".into(),
                function: "on_request".into(),
            },
            &mut request,
            &variables,
        );
        assert!(matches!(request_outcome.status, HookStatus::Success));
        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.headers[0], KeyValue::new("X-Custom", "1"));
        assert_eq!(
            request.auth.as_ref().and_then(|auth| auth.kind),
            Some(AuthKind::Basic)
        );
        assert_eq!(
            request_outcome.effects,
            vec![Effect::Notify {
                message: "sending".into(),
                severity: Severity::Warning
            }]
        );

        let response_outcome = engine.run_on_response(
            &ScriptRef {
                path: "hooks.js".into(),
                function: "on_response".into(),
            },
            &response(),
            &request,
            &variables,
        );
        assert!(matches!(response_outcome.status, HookStatus::Success));
        assert_eq!(response_outcome.logs[0].text, "200 application/json");
        assert_eq!(
            response_outcome.effects[0],
            Effect::SetVariable {
                name: "id".into(),
                value: "7".into()
            }
        );
    }

    #[test]
    fn adapts_arguments_to_declared_function_length() {
        let directory = TestDirectory::new();
        directory.write(
            "arity.js",
            r#"
                export function setup() {
                    if (arguments.length !== 0) throw new Error("setup arity");
                }
                export function on_request(request) {
                    if (arguments.length !== 1 || request.method !== "GET") throw new Error("request arity");
                }
                export function on_response(response, rusting) {
                    if (arguments.length !== 2 || response.status !== 200 || rusting.response !== response) {
                        throw new Error("response arity");
                    }
                }
            "#,
        );
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let variables = Variables::new();
        let mut request = RequestModel::default();

        assert!(matches!(
            engine
                .run_setup(
                    &ScriptRef {
                        path: "arity.js".into(),
                        function: "setup".into()
                    },
                    &variables
                )
                .status,
            HookStatus::Success
        ));
        assert!(matches!(
            engine
                .run_on_request(
                    &ScriptRef {
                        path: "arity.js".into(),
                        function: "on_request".into()
                    },
                    &mut request,
                    &variables
                )
                .status,
            HookStatus::Success
        ));
        assert!(matches!(
            engine
                .run_on_response(
                    &ScriptRef {
                        path: "arity.js".into(),
                        function: "on_response".into()
                    },
                    &response(),
                    &request,
                    &variables
                )
                .status,
            HookStatus::Success
        ));
    }

    #[test]
    fn invalid_request_conversion_is_atomic() {
        let directory = TestDirectory::new();
        directory.write("invalid.js", "export function on_request(request) { request.method = 'NOPE'; request.name = 'changed'; }");
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let mut request = RequestModel {
            name: "original".into(),
            ..RequestModel::default()
        };
        let before = request.clone();

        let outcome = engine.run_on_request(
            &ScriptRef {
                path: "invalid.js".into(),
                function: "on_request".into(),
            },
            &mut request,
            &Variables::new(),
        );
        assert!(matches!(outcome.status, HookStatus::Error(_)));
        assert_eq!(request, before);
    }

    #[test]
    fn response_and_variables_are_read_only() {
        let directory = TestDirectory::new();
        directory.write(
            "readonly.js",
            r#"
                export function setup(rusting) { rusting.variables.X = "changed"; }
                export function on_response(response) { response.status = 201; }
            "#,
        );
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let variables_outcome = engine.run_setup(
            &ScriptRef {
                path: "readonly.js".into(),
                function: "setup".into(),
            },
            &Variables::from([("X".into(), "original".into())]),
        );
        assert!(matches!(variables_outcome.status, HookStatus::Error(_)));
        let outcome = engine.run_on_response(
            &ScriptRef {
                path: "readonly.js".into(),
                function: "on_response".into(),
            },
            &response(),
            &RequestModel::default(),
            &Variables::new(),
        );
        assert!(matches!(outcome.status, HookStatus::Error(_)));
    }

    #[test]
    fn rejects_missing_outside_invalid_and_throwing_scripts() {
        let directory = TestDirectory::new();
        let outside = TestDirectory::new();
        fs::write(outside.0.join("outside.js"), "export function setup() {}")
            .expect("write outside script");
        directory.write("missing-export.js", "export function other() {}");
        directory.write("invalid-syntax.js", "export function setup( {");
        directory.write(
            "throws.js",
            "export function setup() { throw new Error('boom'); }",
        );
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let variables = Variables::new();

        let missing_file = engine.run_setup(
            &ScriptRef {
                path: "absent.js".into(),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(
            matches!(&missing_file.status, HookStatus::Error(message) if message.contains("Script not found"))
        );
        let outside_result = engine.run_setup(
            &ScriptRef {
                path: outside.0.join("outside.js"),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(
            matches!(&outside_result.status, HookStatus::Error(message) if message.contains("escapes the collection"))
        );
        let missing_function = engine.run_setup(
            &ScriptRef {
                path: "missing-export.js".into(),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(
            matches!(&missing_function.status, HookStatus::Error(message) if message.contains("no exported function"))
        );
        let invalid = engine.run_setup(
            &ScriptRef {
                path: "invalid-syntax.js".into(),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(
            matches!(&invalid.status, HookStatus::Error(message) if message.contains("could not be loaded"))
        );
        let threw = engine.run_setup(
            &ScriptRef {
                path: "throws.js".into(),
                function: "setup".into(),
            },
            &variables,
        );
        assert!(matches!(&threw.status, HookStatus::Error(message) if message.contains("boom")));
    }

    #[test]
    fn invalidation_re_evaluates_only_the_requested_module() {
        let directory = TestDirectory::new();
        let script = directory.write(
            "cached.js",
            "globalThis.moduleLoads = (globalThis.moduleLoads || 0) + 1; export function setup() { console.log(globalThis.moduleLoads); }",
        );
        let mut engine = Engine::new(directory.0.clone()).expect("engine");
        let variables = Variables::new();

        assert_eq!(engine.run_setup(&script, &variables).logs[0].text, "1");
        fs::write(
            directory.0.join("cached.js"),
            "globalThis.moduleLoads += 10; export function setup() { console.log(globalThis.moduleLoads); }",
        ).expect("rewrite script");
        assert_eq!(engine.run_setup(&script, &variables).logs[0].text, "1");
        engine.invalidate(Path::new("cached.js"));
        assert_eq!(engine.run_setup(&script, &variables).logs[0].text, "11");
        engine.invalidate_all();
        assert_eq!(engine.run_setup(&script, &variables).logs[0].text, "21");
    }

    #[test]
    fn interrupts_an_infinite_hook_at_the_budget() {
        let directory = TestDirectory::new();
        let script = directory.write("timeout.js", "export function setup() { while (true) {} }");
        let mut engine = Engine::new(directory.0.clone()).expect("engine");

        let outcome = engine.run_setup(&script, &Variables::new());
        assert!(
            matches!(&outcome.status, HookStatus::Error(message) if message.contains(&HOOK_BUDGET_MS.to_string()))
        );
    }
}
