//! The host objects installed for each JavaScript hook invocation.

use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Coerced, Ctx, Error, Function, Object, Value,
    function::{Opt, Rest},
};
use rusting_core::Variables;

use crate::types::{Effect, LogLine, Severity, Stream};

/// Per-invocation output shared by the Rust callbacks owned by QuickJS.
#[derive(Clone, Default)]
pub(crate) struct InvocationOutput {
    logs: Rc<RefCell<Vec<LogLine>>>,
    effects: Rc<RefCell<Vec<Effect>>>,
}

impl InvocationOutput {
    pub(crate) fn logs(&self) -> Vec<LogLine> {
        self.logs.borrow().clone()
    }

    pub(crate) fn effects(&self) -> Vec<Effect> {
        self.effects.borrow().clone()
    }
}

/// Install `console` and build the `rusting` object passed to a hook.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    variables: &Variables,
    request: Option<Value<'js>>,
    response: Option<Value<'js>>,
    output: &InvocationOutput,
) -> rquickjs::Result<Object<'js>> {
    install_console(ctx, output)?;

    let variables_json = serde_json::to_string(variables)
        .map_err(|error| Error::new_into_js_message("Variables", "object", error.to_string()))?;
    let variables_value = ctx.json_parse(variables_json)?;
    freeze(ctx, variables_value.clone())?;

    let rusting = Object::new(ctx.clone())?;
    rusting.set("variables", variables_value)?;
    rusting.set(
        "request",
        request.unwrap_or_else(|| Value::new_null(ctx.clone())),
    )?;
    rusting.set(
        "response",
        response.unwrap_or_else(|| Value::new_null(ctx.clone())),
    )?;

    let effects = output.effects.clone();
    rusting.set(
        "setVariable",
        Function::new(
            ctx.clone(),
            move |name: Coerced<String>, value: Coerced<String>| {
                effects.borrow_mut().push(Effect::SetVariable {
                    name: name.0,
                    value: value.0,
                });
            },
        )?,
    )?;

    let effects = output.effects.clone();
    rusting.set(
        "clearVariable",
        Function::new(ctx.clone(), move |name: Coerced<String>| {
            effects
                .borrow_mut()
                .push(Effect::ClearVariable { name: name.0 });
        })?,
    )?;

    let effects = output.effects.clone();
    rusting.set(
        "clearAllVariables",
        Function::new(ctx.clone(), move || {
            effects.borrow_mut().push(Effect::ClearAllVariables);
        })?,
    )?;

    let effects = output.effects.clone();
    rusting.set(
        "notify",
        Function::new(
            ctx.clone(),
            move |message: Coerced<String>, severity: Opt<Coerced<String>>| {
                let severity = match severity.0.as_ref().map(|value| value.0.as_str()) {
                    None | Some("information") => Severity::Information,
                    Some("warning") => Severity::Warning,
                    Some("error") => Severity::Error,
                    Some(other) => {
                        return Err(Error::new_from_js_message(
                            "string",
                            "severity",
                            format!("unknown severity '{other}'"),
                        ));
                    }
                };
                effects.borrow_mut().push(Effect::Notify {
                    message: message.0,
                    severity,
                });
                Ok(())
            },
        )?,
    )?;

    Ok(rusting)
}

/// Freeze the response and its nested headers object. Modules execute in strict
/// mode, so attempts to mutate either object throw instead of failing silently.
pub(crate) fn freeze_response<'js>(ctx: &Ctx<'js>, response: Value<'js>) -> rquickjs::Result<()> {
    let freeze_response: Function =
        ctx.eval("(response) => { Object.freeze(response.headers); Object.freeze(response); }")?;
    freeze_response.call((response,))
}

fn install_console<'js>(ctx: &Ctx<'js>, output: &InvocationOutput) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    console.set(
        "log",
        console_function(ctx, output.logs.clone(), Stream::Out)?,
    )?;
    console.set(
        "info",
        console_function(ctx, output.logs.clone(), Stream::Out)?,
    )?;
    console.set(
        "warn",
        console_function(ctx, output.logs.clone(), Stream::Err)?,
    )?;
    console.set(
        "error",
        console_function(ctx, output.logs.clone(), Stream::Err)?,
    )?;
    ctx.globals().set("console", console)
}

fn console_function<'js>(
    ctx: &Ctx<'js>,
    logs: Rc<RefCell<Vec<LogLine>>>,
    stream: Stream,
) -> rquickjs::Result<Function<'js>> {
    Function::new(ctx.clone(), move |arguments: Rest<Coerced<String>>| {
        logs.borrow_mut().push(LogLine {
            stream,
            text: arguments
                .0
                .into_iter()
                .map(|argument| argument.0)
                .collect::<Vec<_>>()
                .join(" "),
        });
    })
}

fn freeze<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<()> {
    let freeze: Function = ctx.eval("(value) => Object.freeze(value)")?;
    freeze.call((value,))
}
