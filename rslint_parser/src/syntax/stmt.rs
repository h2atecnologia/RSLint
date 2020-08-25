//! Statements, these include `if`, `while`, `for`, `;`, and more.
//!
//! See the [ECMAScript spec](https://www.ecma-international.org/ecma-262/5.1/#sec-12).

use super::decl::{class_decl, function_decl};
use super::expr::{assign_expr, expr, primary_expr, STARTS_EXPR};
use super::pat::*;
use super::util::{
    check_for_stmt_declarators, check_label_use, check_lhs, check_var_decl_bound_names,
};
use super::program::{import_decl, export_decl};
use crate::{SyntaxKind::*, *};

pub const STMT_RECOVERY_SET: TokenSet = token_set![
    L_CURLY,
    VAR_KW,
    FUNCTION_KW,
    IF_KW,
    FOR_KW,
    DO_KW,
    WHILE_KW,
    CONTINUE_KW,
    BREAK_KW,
    RETURN_KW,
    WITH_KW,
    SWITCH_KW,
    THROW_KW,
    TRY_KW,
    DEBUGGER_KW,
    FUNCTION_KW
];

pub const FOLLOWS_LET: TokenSet = token_set![T!['{'], T!['['], T![ident], T![yield], T![await]];

/// Consume an explicit semicolon, or try to automatically insert one,
/// or add an error to the parser if there was none and it could not be inserted
pub fn semi(p: &mut Parser, err_range: Range<usize>) {
    if p.eat(T![;]) || p.at(EOF) {
        return;
    }
    if !p.has_linebreak_before_n(0) {
        let err = p
            .err_builder(
                "Expected a semicolon or an implicit semicolon after a statement, but found none",
            )
            .primary(
                p.cur_tok().range,
                "An explicit or implicit semicolon is expected here...",
            )
            .secondary(err_range, "...Which is required to end this statement");

        p.error(err);
    }
}

/// A generic statement such as a block, if, while, with, etc
pub fn stmt(p: &mut Parser) -> Option<CompletedMarker> {
    Some(match p.cur() {
        T![;] => empty_stmt(p),
        T!['{'] => block_stmt(p, false),
        T![if] => if_stmt(p),
        T![with] => with_stmt(p),
        T![while] => while_stmt(p),
        T![var] | T![const] => var_decl(p, false),
        T![for] => for_stmt(p),
        T![do] => do_stmt(p),
        T![switch] => switch_stmt(p),
        T![try] => try_stmt(p),
        T![return] => return_stmt(p),
        T![break] => break_stmt(p),
        T![continue] => continue_stmt(p),
        T![throw] => throw_stmt(p),
        T![debugger] => debugger_stmt(p),
        T![function] => {
            let m = p.start();
            // TODO: Should we change this to fn_expr if there is no name?
            function_decl(p, m)
        },
        T![class] => {
            class_decl(p)
        }
        T![ident]
            if p.cur_src() == "async"
                && p.nth_at(1, T![function])
                && !p.has_linebreak_before_n(1) =>
        {
            let m = p.start();
            p.bump_any();
            function_decl(
                &mut *p.with_state(ParserState {
                    in_async: true,
                    ..p.state.clone()
                }),
                m,
            )
        }
        T![ident] if p.cur_src() == "let" && FOLLOWS_LET.contains(p.nth(1)) => var_decl(p, false),
        _ if p.at_ts(STARTS_EXPR) => {
            let start = p.cur_tok().range.start;
            let expr = expr(p)?;
            // Labelled stmt
            if expr.kind() == NAME && p.at(T![:]) {
                // Its not possible to have a name without an inner ident token
                let name = p.parse_marker::<ast::Name>(&expr).ident_token()
                    .expect("Tried to get the ident of a name node, but there was no ident. This is erroneous");
                if let Some(range) = p.state.labels.get(name.text().as_str()) {
                    let err = p
                        .err_builder("Duplicate statement labels are not allowed")
                        .secondary(
                            range.to_owned(),
                            &format!("`{}` is first used as a label here", name.text().as_str()),
                        )
                        .primary(
                            p.cur_tok().range,
                            &format!(
                                "a second use of `{}` here is not allowed",
                                name.text().as_str()
                            ),
                        );

                    p.error(err);
                } else {
                    p.state
                        .labels
                        .insert(name.text().to_string(), name.text_range().into());
                }

                let m = expr.precede(p);
                p.bump_any();
                stmt(p);
                m.complete(p, LABELLED_STMT)
            } else {
                let m = expr.precede(p);
                semi(p, start..p.cur_tok().range.end);
                m.complete(p, EXPR_STMT)
            }
        }
        _ => {
            let err = p
                .err_builder("Expected a statement, but found none")
                .primary(p.cur_tok().range, "Expected a statement here");

            p.err_recover(err, STMT_RECOVERY_SET);
            return None;
        }
    })
}

/// A debugger statement such as `debugger;`
pub fn debugger_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let range = p.cur_tok().range;
    p.expect(T![debugger]);
    semi(p, range);
    m.complete(p, DEBUGGER_STMT)
}

/// A throw statement such as `throw new Error("uh oh");`
pub fn throw_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let start = p.cur_tok().range.start;
    p.expect(T![throw]);
    if p.has_linebreak_before_n(0) {
        let mut err = p
            .err_builder(
                "Linebreaks between a throw statement and the error to be thrown are not allowed",
            )
            .primary(p.cur_tok().range, "A linebreak is not allowed here");

        if p.at_ts(STARTS_EXPR) {
            err = err.secondary(p.cur_tok().range, "Help: did you mean to throw this?");
        }

        p.error(err);
    } else {
        expr(p);
    }
    semi(p, start..p.cur_tok().range.end);
    m.complete(p, THROW_STMT)
}

/// A break statement with an optional label such as `break a;`
pub fn break_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let start = p.cur_tok().range.start;
    p.expect(T![break]);
    if !p.has_linebreak_before_n(0) && p.at(T![ident]) {
        let label = primary_expr(p).unwrap();
        check_label_use(p, &label);
    }
    semi(p, start..p.cur_tok().range.end);

    if !p.state.break_allowed && p.state.labels.is_empty() {
        let err = p
            .err_builder("Invalid break not inside of a switch, loop, or labelled statement")
            .primary(
                start..p.cur_tok().range.end,
                "This break statement is invalid in this context",
            );

        p.error(err);
    }

    m.complete(p, BREAK_STMT)
}

/// A continue statement with an optional label such as `continue a;`
pub fn continue_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let start = p.cur_tok().range.start;
    p.expect(T![continue]);
    if !p.has_linebreak_before_n(0) && p.at(T![ident]) {
        let label = primary_expr(p).unwrap();
        check_label_use(p, &label);
    }
    semi(p, start..p.cur_tok().range.end);

    if !p.state.continue_allowed {
        let err = p
            .err_builder("Invalid continue not inside of a loop")
            .primary(
                start..p.cur_tok().range.end,
                "This continue statement is invalid in this context",
            );

        p.error(err);
    }

    m.complete(p, CONTINUE_STMT)
}

/// A return statement with an optional value such as `return a;`
pub fn return_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let start = p.cur_tok().range.start;
    p.expect(T![return]);
    if !p.has_linebreak_before_n(0) && p.at_ts(STARTS_EXPR) {
        expr(p);
    }
    semi(p, start..p.cur_tok().range.end);
    let complete = m.complete(p, RETURN_STMT);

    if !p.state.in_function {
        let err = p
            .err_builder("Illegal return statement outside of a function")
            .primary(complete.range(p), "");

        p.error(err);
    }
    complete
}

/// An empty statement denoted by a single semicolon.
pub fn empty_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![;]);
    m.complete(p, EMPTY_STMT)
}

/// A block statement consisting of statements wrapped in curly brackets.
pub fn block_stmt(p: &mut Parser, function_body: bool) -> CompletedMarker {
    let m = p.start();
    p.expect(T!['{']);
    block_items(p, function_body, false);
    p.expect(T!['}']);
    m.complete(p, BLOCK_STMT)
}

/// Top level items or items inside of a block statement, this also handles module items so we can
/// easily recover from erroneous module declarations in scripts
pub(crate) fn block_items(p: &mut Parser, directives: bool, top_level: bool) {
    let old = p.state.clone();

    let mut could_be_directive = directives;

    while !p.at(EOF) && !p.at(T!['}']) {
        let complete = match p.cur() {
            T![import] => {
                let mut m = import_decl(p);
                if !p.state.is_module {
                    let err = p.err_builder("Illegal use of an import declaration outside of a module")
                        .primary(m.range(p), "not allowed inside scripts")
                        .help("Note: the parser is configured for scripts, not modules");

                    p.error(err);
                    m.change_kind(p, ERROR);
                }
                Some(m)
            },
            T![export] => {
                let mut m = export_decl(p);
                if !p.state.is_module {
                    let err = p.err_builder("Illegal use of an export declaration outside of a module")
                        .primary(m.range(p), "not allowed inside scripts")
                        .help("Note: the parser is configured for scripts, not modules");

                    p.error(err);
                    m.change_kind(p, ERROR);
                }
                Some(m)
            }
            _ => stmt(p),
        };
        
        // Directives are the longest sequence of string literals, so
        // ```
        // function a() {
        //  "aaa";
        //  "use strict"
        // }
        // ```
        // Still makes the function body strict
        if let Some(kind) = complete.map(|x| x.kind()).filter(|_| could_be_directive) {
            match kind {
                EXPR_STMT => {
                    let parsed = p.parse_marker::<ast::ExprStmt>(complete.as_ref().unwrap()).expr();
                    if let Some(LITERAL) = parsed.as_ref().map(|it| it.syntax().kind()) {
                        let unwrapped = parsed.unwrap().syntax().to::<ast::Literal>();
                        if unwrapped.is_string() {
                            if unwrapped.inner_string_text().unwrap() == "use strict" {
                                let range = complete.as_ref().unwrap().range(p).into();
                                // We must do this because we cannot have multiple mutable borrows of p
                                let mut new = p.state.clone();
                                new.strict(p, range, top_level);
                                p.state = new;
                                could_be_directive = false;
                            }
                        } else {
                            could_be_directive = false;
                        }
                    }
                },
                _ => could_be_directive = false
            }
        }
    }
    p.state = old;
}

/// An expression wrapped in parentheses such as `()
pub fn condition(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T!['(']);
    expr(p);
    p.expect(T![')']);
    m.complete(p, CONDITION)
}

/// An if statement such as `if (foo) { bar(); }`
pub fn if_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![if]);
    condition(p);
    stmt(p);
    if p.eat(T![else]) {
        stmt(p);
    }
    m.complete(p, IF_STMT)
}

/// A with statement such as `with (foo) something()`
pub fn with_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![with]);
    condition(p);
    stmt(p);
    m.complete(p, WITH_STMT)
}

/// A while statement such as `while(true) { do_something() }`
pub fn while_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![while]);
    condition(p);
    stmt(&mut *p.with_state(ParserState { break_allowed: true, continue_allowed: true, ..p.state.clone() }));
    m.complete(p, WHILE_STMT)
}

/// A var, const, or let declaration such as `var a = 5, b;` or `let {a, b} = foo;`
pub fn var_decl(p: &mut Parser, no_semi: bool) -> CompletedMarker {
    let m = p.start();
    let start = p.cur_tok().range.start;
    let mut is_const = None;

    match p.cur() {
        T![var] => p.bump_any(),
        T![const] => {
            is_const = Some(p.cur_tok().range);
            p.bump_any()
        }
        T![ident] if p.cur_src() == "let" => p.bump_any(),
        _ => {
            let err = p
                .err_builder(
                    "Expected `var`, `let`, or `const` for a variable declaration, but found none",
                )
                .primary(p.cur_tok(), "");

            p.error(err);
        }
    }

    declarator(p, &is_const, no_semi);

    if p.eat(T![,]) {
        declarator(p, &is_const, no_semi);
        while p.eat(T![,]) {
            declarator(p, &is_const, no_semi);
        }
    }

    if !no_semi {
        semi(p, start..p.cur_tok().range.start);
    }
    let complete = m.complete(p, VAR_DECL);
    check_var_decl_bound_names(p, &complete);
    complete
}

// A single declarator, either `ident` or `ident = assign_expr`
fn declarator(p: &mut Parser, is_const: &Option<Range<usize>>, for_stmt: bool) -> CompletedMarker {
    let m = p.start();
    let pat = pattern(p);

    if p.eat(T![=]) {
        assign_expr(p);
    } else if let Some(ref marker) = pat {
        if marker.kind() != SINGLE_PATTERN {
            let err = p
                .err_builder("Object and Array patterns require initializers")
                .primary(
                    marker.range(p),
                    "this pattern is declared, but it is not given an initialized value",
                );

            p.error(err);
        } else if is_const.is_some() && !for_stmt {
            let err = p
                .err_builder("Const var declarations must have an initialized value")
                .primary(marker.range(p), "this variable needs to be initialized");

            p.error(err);
        }
    }

    m.complete(p, DECLARATOR)
}

// A do.. while statement, such as `do {} while (true)`
pub fn do_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![do]);
    p.state.iteration_stmt(true);
    stmt(p);
    p.state.iteration_stmt(false);
    p.expect(T![while]);
    condition(p);
    m.complete(p, DO_WHILE_STMT)
}

fn for_head(p: &mut Parser) -> SyntaxKind {
    if p.at(T![const]) || p.at(T![var]) || (p.cur_src() == "let" && FOLLOWS_LET.contains(p.nth(1)))
    {
        let decl = var_decl(p, true);

        if p.at(T![in]) || p.cur_src() == "of" {
            let is_in = p.at(T![in]);
            p.bump_any();

            check_for_stmt_declarators(p, &decl);

            for_each_head(p, is_in)
        } else {
            p.expect(T![;]);
            normal_for_head(p);
            FOR_STMT
        }
    } else {
        if p.eat(T![;]) {
            normal_for_head(p);
            return FOR_STMT;
        }
        let complete = expr(p);

        if p.at(T![in]) || p.cur_src() == "of" {
            let is_in = p.at(T![in]);
            p.bump_any();

            if let Some(ref expr) = complete {
                check_lhs(p, p.parse_marker(expr), &complete.unwrap());
            }

            return for_each_head(p, is_in);
        }

        p.expect(T![;]);
        normal_for_head(p);
        FOR_STMT
    }
}

fn for_each_head(p: &mut Parser, is_in: bool) -> SyntaxKind {
    if is_in {
        expr(p);
        FOR_IN_STMT
    } else {
        assign_expr(p);
        FOR_OF_STMT
    }
}

fn normal_for_head(p: &mut Parser) {
    if !p.eat(T![;]) {
        expr(p);
        p.expect(T![;]);
    }

    if !p.at(T![')']) {
        expr(p);
    }
}

/// Either a traditional for statement or a for.. in statement
pub fn for_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![for]);
    p.eat(T![await]);

    p.expect(T!['(']);
    let kind = for_head(p);
    p.expect(T![')']);
    p.state.iteration_stmt(true);
    stmt(p);
    p.state.iteration_stmt(false);
    m.complete(p, kind)
}

// We return the range in case its a default clause so we can report multiple default clauses in a better way
fn switch_clause(p: &mut Parser) -> Option<Range<usize>> {
    let start = p.cur_tok().range.start;
    match p.cur() {
        T![default] => {
            p.bump_any();
            p.expect(T![:]);
            // We stop the range here because we dont want to include the entire clause
            // including the statement list following it
            let end = p.cur_tok().range.end;
            while !p.at_ts(token_set![T![default], T![case], T!['}'], EOF]) {
                stmt(p);
            }
            return Some(start..end);
        }
        T![case] => {
            p.bump_any();
            expr(p);
            p.expect(T![:]);
            while !p.at_ts(token_set![T![default], T![case], T!['}'], EOF]) {
                stmt(p);
            }
        }
        _ => {
            let err = p
                .err_builder(
                    "Expected a `case` or `default` clause in a switch statement, but found none",
                )
                .primary(
                    p.cur_tok().range,
                    "Expected the start to a case or default clause here",
                );

            p.error(err);
        }
    }
    None
}

/// A switch statement such as
///
/// ```js
/// switch (a) {
///     case foo:
///         bar();
/// }
/// ```
pub fn switch_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![switch]);
    condition(p);
    p.expect(T!['{']);
    let mut first_default: Option<Range<usize>> = None;

    while !p.at(EOF) && !p.at(T!['}']) {
        let mut temp = p.with_state(ParserState { break_allowed: true, ..p.state.clone() });
        if let Some(range) = switch_clause(&mut *temp) {
            if let Some(ref err_range) = first_default {
                let err = temp
                    .err_builder(
                        "Multiple default clauses inside of a switch statement are not allowed",
                    )
                    .secondary(
                        err_range.to_owned(),
                        "the first default clause is defined here",
                    )
                    .primary(range, "a second clause here is not allowed");

                temp.error(err);
            } else {
                first_default = Some(range);
            }
        }
    }
    p.expect(T!['}']);
    m.complete(p, SWITCH_STMT)
}

fn catch_clause(p: &mut Parser) {
    let m = p.start();
    p.expect(T![catch]);

    // This allows u to recover from `catch something) {` more effectively
    if p.eat(T!['(']) || !p.at(T!['{']) {
        if !p.at(IDENT) {
            let err = p
                .err_builder(
                    "Expected an identifier for the error in a catch clause, but found none",
                )
                .primary(p.cur_tok().range, "Expected an identifier here");

            p.error(err);
        } else {
            let name = p.start();
            p.bump_any();
            name.complete(p, NAME);
        }

        p.expect(T![')']);
    }

    block_stmt(p, false);
    m.complete(p, CATCH_CLAUSE);
}

/// A try statement such as
///
/// ```js
/// try {
///     something();
/// } catch (a) {
///
/// }
/// ```
pub fn try_stmt(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(T![try]);
    block_stmt(p, false);
    if p.at(T![catch]) {
        catch_clause(p);
    }
    if p.at(T![finally]) {
        let finalizer = p.start();
        p.bump_any();
        block_stmt(p, false);
        finalizer.complete(p, FINALIZER);
    }
    m.complete(p, TRY_STMT)
}
