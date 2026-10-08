//! Entry control flow and transitions parsed on the shared source cursor.

use super::{AuthoredEntryRoute, AuthoredEntryStatement, AuthoredSuccessor, NamePath, Origin, Parser, RouteId, TokenKind, word};
use crate::error::Result;

impl<'src> Parser<'src> {
    pub(super) fn parse_entry_statement_sequence(&mut self) -> Result<Vec<AuthoredEntryStatement<'src>>> {
        let mut statements = Vec::new();
        while !self.check_symbol('}') && !self.is_eof() {
            if self.consume_symbol(';') {
                continue;
            }
            statements.push(self.parse_entry_statement()?);
        }
        Ok(statements)
    }

    fn parse_entry_statement(&mut self) -> Result<AuthoredEntryStatement<'src>> {
        let start = self.current().span.start;
        if self.braced_assignment_start() {
            return Ok(AuthoredEntryStatement::Sil(Box::new(self.parse_sil_statement()?)));
        }
        if self.consume_symbol('{') {
            let statements = self.parse_entry_statement_sequence()?;
            self.expect_symbol('}')?;
            return Ok(AuthoredEntryStatement::Block { statements, span: self.sil_span(start, self.previous().span.end) });
        }
        if self.consume_ident(word::IF) {
            self.expect_symbol('(')?;
            let condition = self.parse_expression()?;
            self.expect_symbol(')')?;
            let then_branch = Box::new(self.parse_entry_statement()?);
            let else_branch = if self.consume_ident(word::ELSE) { Some(Box::new(self.parse_entry_statement()?)) } else { None };
            return Ok(AuthoredEntryStatement::If {
                condition,
                then_branch,
                else_branch,
                span: self.sil_span(start, self.previous().span.end),
            });
        }
        if self.consume_ident(word::BECOME) {
            let routes = self.parse_entry_routes()?;
            return Ok(AuthoredEntryStatement::Become { routes, span: self.sil_span(start, self.previous().span.end) });
        }
        if self.check_ident(word::REQUIRE)
            && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Ident(_)))
            && matches!(self.tokens.get(self.pos + 2).map(|token| &token.kind), Some(TokenKind::Symbol('.')))
            && self.peek_ident(3, word::OUTPUTS)
            && self.peek_ident(4, word::BECOME)
        {
            self.expect_ident(word::REQUIRE)?;
            let group_start = self.current().span.start;
            let group = self.expect_any_ident()?;
            let group_end = self.previous().span.end;
            self.expect_symbol('.')?;
            self.expect_ident(word::OUTPUTS)?;
            self.expect_ident(word::BECOME)?;
            let routes = self.parse_entry_routes()?;
            return Ok(AuthoredEntryStatement::ForeignBecome {
                group: NamePath {
                    segments: vec![group],
                    origin: Origin::Authored { source: self.file.id, start: group_start, end: group_end },
                },
                routes,
                span: self.sil_span(start, self.previous().span.end),
            });
        }
        Ok(AuthoredEntryStatement::Sil(Box::new(self.parse_sil_statement()?)))
    }

    fn parse_entry_routes(&mut self) -> Result<Vec<AuthoredEntryRoute<'src>>> {
        if self.consume_symbol('{') {
            let mut routes = Vec::new();
            while !self.check_symbol('}') && !self.is_eof() {
                if self.check_ident(word::BECOME) {
                    return Err(self.error("nested `become` blocks are not supported yet"));
                }
                routes.push(self.parse_entry_route()?);
                self.expect_list_separator_or_end('}')?;
            }
            self.expect_symbol('}')?;
            self.consume_symbol(';');
            Ok(routes)
        } else {
            let route = self.parse_entry_route()?;
            self.consume_symbol(';');
            Ok(vec![route])
        }
    }

    fn parse_entry_route(&mut self) -> Result<AuthoredEntryRoute<'src>> {
        let output_start = self.current().span.start;
        let output = self.expect_any_ident()?;
        let output_end = self.previous().span.end;
        if !matches!(self.current().kind, TokenKind::LeftArrow) {
            return Err(self.error("every `become` route must name its output with `output <- successor`"));
        }
        self.advance();
        let successor_start = self.current().span.start;
        let successor = if self.check_ident(word::SELF)
            && matches!(
                self.tokens.get(self.pos + 1).map(|token| &token.kind),
                Some(TokenKind::Symbol(',' | ';' | '}') | TokenKind::Eof)
            ) {
            self.advance();
            AuthoredSuccessor::SelfRef {
                origin: Origin::Authored { source: self.file.id, start: successor_start, end: self.previous().span.end },
            }
        } else {
            let mut depth = 0usize;
            let mut open = None;
            for index in self.pos..self.tokens.len() {
                match self.tokens[index].kind {
                    TokenKind::Symbol('(') if depth == 0 => {
                        open = Some(index);
                        break;
                    }
                    TokenKind::Symbol('[' | '{' | '<') => depth += 1,
                    TokenKind::Symbol(']' | '}' | '>') if depth > 0 => depth -= 1,
                    TokenKind::Symbol(',' | ';' | ')' | '}') | TokenKind::Eof if depth == 0 => break,
                    _ => {}
                }
            }
            let open = open.ok_or_else(|| self.error("expected `(` after become target"))?;
            let many = open >= self.pos + 2
                && matches!(self.tokens[open - 2].kind, TokenKind::Symbol('['))
                && matches!(self.tokens[open - 1].kind, TokenKind::Symbol(']'));
            let target_end = if many { open - 2 } else { open };
            self.expression_end = Some(target_end);
            let actor = self.parse_expression();
            self.expression_end = None;
            let actor = actor?;
            if self.pos != target_end {
                return Err(self.error("expected `(` after become target"));
            }
            if many {
                self.expect_symbol('[')?;
                self.expect_symbol(']')?;
            }
            self.expect_symbol('(')?;
            let state = self.parse_expression()?;
            self.expect_symbol(')')?;
            AuthoredSuccessor::Constructed { actor: Box::new(actor), state: Box::new(state), many }
        };
        let id = RouteId(self.next_route_id);
        self.next_route_id += 1;
        Ok(AuthoredEntryRoute {
            id,
            output: NamePath {
                segments: vec![output],
                origin: Origin::Authored { source: self.file.id, start: output_start, end: output_end },
            },
            successor,
        })
    }
}

#[cfg(test)]
mod tests;
