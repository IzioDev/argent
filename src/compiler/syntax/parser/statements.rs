//! Ordinary Sil statements parsed from the same source tokens as declarations.

use silverscript_lang::ast::{self as sil, ExprKind, ParamAst, Statement, StructBindingAst};

use super::{Parser, TokenKind};
use crate::error::Result;

impl<'src> Parser<'src> {
    pub(super) fn parse_sil_statement_sequence(&mut self) -> Result<Vec<Statement<'src>>> {
        let mut statements = Vec::new();
        while !self.check_symbol('}') && !self.is_eof() {
            if self.consume_symbol(';') {
                continue;
            }
            statements.push(self.parse_sil_statement()?);
        }
        Ok(statements)
    }

    pub(super) fn parse_sil_statement(&mut self) -> Result<Statement<'src>> {
        let start = self.current().span.start;
        if self.entry_loop_depth > 0
            && (self.check_ident("become")
                || (self.check_ident("require")
                    && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Ident(_)))
                    && matches!(self.tokens.get(self.pos + 2).map(|token| &token.kind), Some(TokenKind::Symbol('.')))
                    && self.peek_ident(3, "outputs")
                    && self.peek_ident(4, "become")))
        {
            return Err(self.error("transitions are not allowed inside ordinary loops"));
        }
        if self.braced_assignment_start() {
            return self.parse_struct_destructure(start, String::new(), self.sil_span(start, start));
        }
        if self.consume_symbol('{') {
            let body = self.parse_sil_statement_sequence()?;
            self.expect_symbol('}')?;
            return Ok(Statement::Block { body, span: self.sil_span(start, self.previous().span.end) });
        }
        if self.consume_ident("if") {
            self.expect_symbol('(')?;
            let condition = self.parse_expression()?;
            self.expect_symbol(')')?;
            let (then_branch, then_span) = self.parse_sil_block_or_statement()?;
            let (else_branch, else_span) = if self.consume_ident("else") {
                let (branch, span) = self.parse_sil_block_or_statement()?;
                (Some(branch), Some(span))
            } else {
                (None, None)
            };
            return Ok(Statement::If {
                condition,
                then_branch,
                else_branch,
                span: self.sil_span(start, self.previous().span.end),
                then_span,
                else_span,
            });
        }
        if self.consume_ident("for") {
            self.expect_symbol('(')?;
            let ident_span = self.sil_span(self.current().span.start, self.current().span.end);
            let ident = self.expect_any_ident()?;
            self.expect_symbol(',')?;
            let from = self.parse_expression()?;
            self.expect_symbol(',')?;
            let end = self.parse_expression()?;
            self.expect_symbol(',')?;
            let max_iterations = self.parse_expression()?;
            self.expect_symbol(')')?;
            self.entry_loop_depth += 1;
            let parsed_body = self.parse_sil_block_or_statement();
            self.entry_loop_depth -= 1;
            let (body, body_span) = parsed_body?;
            return Ok(Statement::For {
                ident,
                start: from,
                end,
                max_iterations,
                body,
                span: self.sil_span(start, self.previous().span.end),
                ident_span,
                body_span,
            });
        }
        if self.consume_ident("return") {
            let mut exprs = Vec::new();
            if self.consume_symbol('(') {
                while !self.check_symbol(')') {
                    exprs.push(self.parse_expression()?);
                    if !self.consume_symbol(',') {
                        break;
                    }
                }
                self.expect_symbol(')')?;
            } else if !self.check_symbol(';') {
                exprs.push(self.parse_expression()?);
                while self.consume_symbol(',') {
                    exprs.push(self.parse_expression()?);
                }
            }
            self.expect_symbol(';')?;
            return Ok(Statement::Return { exprs, span: self.sil_span(start, self.previous().span.end) });
        }
        if self.consume_ident("require") {
            self.expect_symbol('(')?;
            let lock_target = if (self.check_ident("this") || self.check_ident("tx"))
                && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Symbol('.')))
                && matches!(self.tokens.get(self.pos + 2).map(|token| &token.kind), Some(TokenKind::Ident(_)))
                && matches!(self.tokens.get(self.pos + 3).map(|token| &token.kind), Some(TokenKind::Symbol('>')))
                && matches!(self.tokens.get(self.pos + 4).map(|token| &token.kind), Some(TokenKind::Symbol('=')))
            {
                let target_start = self.current().span.start;
                let root = self.expect_any_ident()?;
                self.expect_symbol('.')?;
                let field = self.expect_any_ident()?;
                let target = format!("{root}.{field}");
                if matches!(target.as_str(), "this.ageDaa" | "tx.daa" | "tx.time") {
                    let target_span = self.sil_span(target_start, self.previous().span.end);
                    self.expect_symbol('>')?;
                    self.expect_symbol('=')?;
                    Some((target, target_span))
                } else {
                    self.pos -= 3;
                    None
                }
            } else {
                None
            };
            let expr = self.parse_expression()?;
            let (message, message_span) = if self.consume_symbol(',') {
                let span = self.sil_span(self.current().span.start, self.current().span.end);
                (Some(self.expect_string()?), Some(span))
            } else {
                (None, None)
            };
            self.expect_symbol(')')?;
            self.expect_symbol(';')?;
            let span = self.sil_span(start, self.previous().span.end);
            return Ok(match lock_target {
                Some((target, target_span)) if target == "this.ageDaa" => {
                    Statement::RequireAgeDaa { expr, message, span, target_span, message_span }
                }
                Some((target, target_span)) if target == "tx.daa" => {
                    Statement::RequireTxDaa { expr, message, span, target_span, message_span }
                }
                Some((_, target_span)) => Statement::RequireTxTime { expr, message, span, target_span, message_span },
                None => Statement::Require { expr, message, span, message_span },
            });
        }
        if self.check_ident("console")
            && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Symbol('.')))
            && matches!(self.tokens.get(self.pos + 2).map(|token| &token.kind), Some(TokenKind::Ident(name)) if name == "log")
        {
            self.advance();
            self.expect_symbol('.')?;
            self.expect_ident("log")?;
            let args = self.parse_expression_args()?;
            self.expect_symbol(';')?;
            return Ok(Statement::Console { args, span: self.sil_span(start, self.previous().span.end) });
        }
        if self.check_symbol('(') {
            let bindings = self.parse_parenthesized_bindings()?;
            self.expect_symbol('=')?;
            let expr = self.parse_expression()?;
            self.expect_symbol(';')?;
            let span = self.sil_span(start, self.previous().span.end);
            if let ExprKind::Call { name, args, name_span } = expr.kind {
                return Ok(Statement::FunctionCallAssign { bindings, name, args, span, name_span });
            }
            if let [left, right] = bindings.as_slice() {
                return Ok(Statement::TupleAssignment {
                    left_type_ref: left.type_ref.clone(),
                    left_name: left.name.clone(),
                    right_type_ref: right.type_ref.clone(),
                    right_name: right.name.clone(),
                    expr,
                    span,
                    left_type_span: left.type_span,
                    left_name_span: left.name_span,
                    right_type_span: right.type_span,
                    right_name_span: right.name_span,
                });
            }
            return Err(self.file.error_at(start, "tuple assignment needs two bindings"));
        }

        let saved = self.pos;
        if matches!(self.current().kind, TokenKind::Ident(_)) {
            let type_start = self.current().span.start;
            let type_ref = self.parse_sil_type()?;
            let type_span = self.sil_span(type_start, self.previous().span.end);
            if self.check_symbol('{') {
                return self.parse_struct_destructure(start, type_ref.base.type_name(), type_span);
            }
            let mut modifiers = Vec::new();
            let mut modifier_spans = Vec::new();
            while self.check_ident("constant") {
                modifier_spans.push(self.sil_span(self.current().span.start, self.current().span.end));
                modifiers.push(self.expect_any_ident()?);
            }
            if matches!(self.current().kind, TokenKind::Ident(_)) {
                let name_span = self.sil_span(self.current().span.start, self.current().span.end);
                let name = self.expect_any_ident()?;
                if self.consume_symbol(',') {
                    let right_type_start = self.current().span.start;
                    let right_type_ref = self.parse_sil_type()?;
                    let right_type_span = self.sil_span(right_type_start, self.previous().span.end);
                    let right_name_span = self.sil_span(self.current().span.start, self.current().span.end);
                    let right_name = self.expect_any_ident()?;
                    self.expect_symbol('=')?;
                    let expr = self.parse_expression()?;
                    self.expect_symbol(';')?;
                    return Ok(Statement::TupleAssignment {
                        left_type_ref: type_ref,
                        left_name: name,
                        right_type_ref,
                        right_name,
                        expr,
                        span: self.sil_span(start, self.previous().span.end),
                        left_type_span: type_span,
                        left_name_span: name_span,
                        right_type_span,
                        right_name_span,
                    });
                }
                if self.check_symbol('=') || self.check_symbol(';') {
                    let expr = if self.consume_symbol('=') {
                        if self.check_symbol('{') {
                            Some(self.parse_braced_literal(type_ref.base.type_name(), type_span, self.current().span.start)?)
                        } else {
                            Some(self.parse_expression()?)
                        }
                    } else {
                        None
                    };
                    self.expect_symbol(';')?;
                    return Ok(Statement::VariableDefinition {
                        type_ref,
                        modifiers,
                        name,
                        expr,
                        span: self.sil_span(start, self.previous().span.end),
                        type_span,
                        modifier_spans,
                        name_span,
                    });
                }
            }
        }
        self.pos = saved;
        if matches!(self.current().kind, TokenKind::Ident(_))
            && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Symbol('=')))
        {
            let name_span = self.sil_span(self.current().span.start, self.current().span.end);
            let name = self.expect_any_ident()?;
            self.expect_symbol('=')?;
            let expr = self.parse_expression()?;
            self.expect_symbol(';')?;
            return Ok(Statement::Assign { name, expr, span: self.sil_span(start, self.previous().span.end), name_span });
        }
        let expr = self.parse_expression()?;
        self.expect_symbol(';')?;
        let (name, args, name_span) = match expr.kind {
            ExprKind::Call { name, args, name_span } => (name, args, name_span),
            ExprKind::Identifier(name) if name.contains("::") => (name, Vec::new(), expr.span),
            _ => return Err(self.file.error_at(start, "expected Sil call, assignment, or definition")),
        };
        Ok(Statement::FunctionCall { name, args, span: self.sil_span(start, self.previous().span.end), name_span })
    }

    fn parse_sil_block_or_statement(&mut self) -> Result<(Vec<Statement<'src>>, sil::Span<'src>)> {
        let start = self.current().span.start;
        if self.consume_symbol('{') {
            let body = self.parse_sil_statement_sequence()?;
            self.expect_symbol('}')?;
            Ok((body, self.sil_span(start, self.previous().span.end)))
        } else {
            let statement = self.parse_sil_statement()?;
            Ok((vec![statement], self.sil_span(start, self.previous().span.end)))
        }
    }

    fn parse_parenthesized_bindings(&mut self) -> Result<Vec<ParamAst<'src>>> {
        self.expect_symbol('(')?;
        let mut bindings = Vec::new();
        while !self.check_symbol(')') {
            let start = self.current().span.start;
            let type_ref = self.parse_sil_type()?;
            let type_span = self.sil_span(start, self.previous().span.end);
            let name_span = self.sil_span(self.current().span.start, self.current().span.end);
            let name = self.expect_any_ident()?;
            bindings.push(ParamAst { type_ref, name, span: self.sil_span(start, self.previous().span.end), type_span, name_span });
            if !self.consume_symbol(',') {
                break;
            }
        }
        self.expect_symbol(')')?;
        Ok(bindings)
    }

    fn parse_struct_destructure(
        &mut self,
        start: usize,
        struct_name: String,
        struct_name_span: sil::Span<'src>,
    ) -> Result<Statement<'src>> {
        self.expect_symbol('{')?;
        let mut bindings = Vec::new();
        while !self.check_symbol('}') {
            let field_start = self.current().span.start;
            let field_name = self.expect_any_ident()?;
            let field_span = self.sil_span(field_start, self.previous().span.end);
            self.expect_symbol(':')?;
            let type_start = self.current().span.start;
            let type_ref = self.parse_sil_type()?;
            let type_span = self.sil_span(type_start, self.previous().span.end);
            let name_span = self.sil_span(self.current().span.start, self.current().span.end);
            let name = self.expect_any_ident()?;
            bindings.push(StructBindingAst {
                field_name,
                type_ref,
                name,
                span: self.sil_span(field_start, self.previous().span.end),
                field_span,
                type_span,
                name_span,
            });
            if !self.consume_symbol(',') {
                break;
            }
        }
        self.expect_symbol('}')?;
        self.expect_symbol('=')?;
        let expr = self.parse_expression()?;
        self.expect_symbol(';')?;
        let span = self.sil_span(start, self.previous().span.end);
        if let ExprKind::Call { name, args, name_span } = expr.kind {
            Ok(Statement::StateFunctionCallAssign {
                target_struct: struct_name,
                bindings,
                name,
                args,
                span,
                target_struct_span: struct_name_span,
                name_span,
            })
        } else {
            Ok(Statement::StructDestructure { struct_name, bindings, expr, span, struct_name_span })
        }
    }

    pub(super) fn braced_assignment_start(&self) -> bool {
        if !self.check_symbol('{') {
            return false;
        }
        let mut depth = 0usize;
        for index in self.pos..self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::Symbol('{') => depth += 1,
                TokenKind::Symbol('}') => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(index + 1).map(|token| &token.kind), Some(TokenKind::Symbol('=')));
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests;
