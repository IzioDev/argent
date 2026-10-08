//! Direct Sil expression construction from the shared Argent token stream.

use chrono::NaiveDateTime;
use silverscript_lang::ast::{
    self as sil, ArrayDim, BinaryOp, Expr, ExprKind, IndexedIntrospectionKind, IntrospectionKind, SplitPart, StateFieldExpr,
    TypeRef as SilTypeRef, UnaryOp, UnarySuffixKind,
};

use super::{Parser, TokenKind};
use crate::error::Result;

#[cfg(test)]
mod tests;

impl<'src> Parser<'src> {
    pub(super) fn parse_expression(&mut self) -> Result<Expr<'src>> {
        self.parse_expression_bp(0)
    }

    fn parse_expression_bp(&mut self, min_bp: u8) -> Result<Expr<'src>> {
        let mut left = self.parse_prefix()?;
        loop {
            if self.expression_end.is_some_and(|end| self.pos >= end) {
                break;
            }
            if min_bp <= 10 && self.check_expression_postfix() {
                left = self.parse_postfix(left)?;
                continue;
            }
            let Some((operator, left_bp, width)) = self.binary_operator() else {
                break;
            };
            if left_bp < min_bp {
                break;
            }
            for _ in 0..width {
                self.advance();
            }
            let right = self.parse_expression_bp(left_bp + 1)?;
            let span = self.sil_span(left.span.start(), right.span.end());
            left = Expr::new(ExprKind::Binary { op: operator, left: Box::new(left), right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expr<'src>> {
        let start = self.current().span.start;
        match self.current().kind.clone() {
            TokenKind::Symbol('!') | TokenKind::Symbol('-') => {
                let op = if self.check_symbol('!') { UnaryOp::Not } else { UnaryOp::Neg };
                self.advance();
                let expr = self.parse_expression_bp(10)?;
                Ok(Expr::new(ExprKind::Unary { op, expr: Box::new(expr) }, self.sil_span(start, self.previous().span.end)))
            }
            TokenKind::Symbol('(') => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect_symbol(')')?;
                Ok(expr)
            }
            TokenKind::Symbol('{') => self.parse_braced_literal(String::new(), self.sil_span(start, start), start),
            TokenKind::Number(raw) => {
                let end = self.current().span.end;
                self.advance();
                self.parse_number_expression(&raw, start, end)
            }
            TokenKind::Str(value) => {
                let end = self.current().span.end;
                self.advance();
                Ok(Expr::new(ExprKind::String(value), self.sil_span(start, end)))
            }
            TokenKind::Ident(name) if name == "true" || name == "false" => {
                let end = self.current().span.end;
                self.advance();
                Ok(Expr::new(ExprKind::Bool(name == "true"), self.sil_span(start, end)))
            }
            TokenKind::Ident(name) if name == "new" => {
                self.advance();
                let name_start = self.current().span.start;
                let name = self.expect_any_qualified_ident()?;
                let name_span = self.sil_span(name_start, self.previous().span.end);
                let args = self.parse_expression_args()?;
                Ok(Expr::new(ExprKind::New { name, args, name_span }, self.sil_span(start, self.previous().span.end)))
            }
            TokenKind::Ident(name)
                if name == "date"
                    && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Symbol('('))) =>
            {
                self.advance();
                self.expect_symbol('(')?;
                let raw = self.expect_string()?;
                self.expect_symbol(')')?;
                let value = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M:%S")
                    .map_err(|_| self.file.error_at(start, format!("invalid date literal `{raw}`")))?;
                Ok(Expr::new(
                    ExprKind::DateLiteral(value.and_utc().timestamp_millis()),
                    self.sil_span(start, self.previous().span.end),
                ))
            }
            TokenKind::Ident(_) => self.parse_name_expression(),
            _ => Err(self.error(format!("expected expression, found {}", self.describe_current()))),
        }
    }

    fn parse_name_expression(&mut self) -> Result<Expr<'src>> {
        let start = self.current().span.start;
        let name = self.expect_any_qualified_ident()?;
        let name_end = self.previous().span.end;
        let name_span = self.sil_span(start, name_end);
        if self.check_symbol('[') && self.type_suffix_has_literal() {
            let (ty, _) = self.parse_sil_type_tail(name)?;
            let type_span = self.sil_span(start, self.previous().span.end);
            if self.check_symbol('{') {
                return self.parse_typed_array(ty, type_span, start);
            }
            if let Some(hex) = self.parse_hex_cast(&ty, type_span, start)? {
                return Ok(hex);
            }
            let args = self.parse_expression_args()?;
            return Ok(Expr::new(
                ExprKind::Call { name: ty.type_name(), args, name_span: type_span },
                self.sil_span(start, self.previous().span.end),
            ));
        }
        if self.check_symbol('(') && self.expression_end != Some(self.pos) {
            let ty = self.parse_sil_type_tail(name.clone())?.0;
            if let Some(hex) = self.parse_hex_cast(&ty, name_span, start)? {
                return Ok(hex);
            }
            let args = self.parse_expression_args()?;
            return Ok(Expr::new(ExprKind::Call { name, args, name_span }, self.sil_span(start, self.previous().span.end)));
        }
        if self.check_symbol('{') && self.expression_end != Some(self.pos) {
            return self.parse_braced_literal(name, name_span, start);
        }
        Ok(Expr::new(ExprKind::Identifier(name), name_span))
    }

    fn type_suffix_has_literal(&self) -> bool {
        let mut index = self.pos;
        let limit = self.expression_end.unwrap_or(self.tokens.len());
        while index < limit && matches!(self.tokens.get(index).map(|token| &token.kind), Some(TokenKind::Symbol('['))) {
            index += 1;
            while index < limit
                && !matches!(self.tokens.get(index).map(|token| &token.kind), Some(TokenKind::Symbol(']') | TokenKind::Eof) | None)
            {
                index += 1;
            }
            if index >= limit || !matches!(self.tokens.get(index).map(|token| &token.kind), Some(TokenKind::Symbol(']'))) {
                return false;
            }
            index += 1;
        }
        index < limit && matches!(self.tokens.get(index).map(|token| &token.kind), Some(TokenKind::Symbol('{' | '(')))
    }

    fn parse_hex_cast(&mut self, ty: &SilTypeRef, type_span: sil::Span<'src>, start: usize) -> Result<Option<Expr<'src>>> {
        let [Some(open), Some(literal), Some(close)] =
            [self.tokens.get(self.pos), self.tokens.get(self.pos + 1), self.tokens.get(self.pos + 2)]
        else {
            return Ok(None);
        };
        let TokenKind::Number(raw) = &literal.kind else {
            return Ok(None);
        };
        if !matches!(open.kind, TokenKind::Symbol('('))
            || !matches!(close.kind, TokenKind::Symbol(')'))
            || !raw.starts_with("0x") && !raw.starts_with("0X")
        {
            return Ok(None);
        }
        let is_byte_array = matches!(ty.base, sil::TypeBase::Byte) && ty.array_dims.len() == 1;
        let fixed_len = ty.base.fixed_byte_sequence_len();
        if !is_byte_array && fixed_len.is_none() {
            return Ok(None);
        }
        let digits = &raw[2..];
        if digits.is_empty() {
            return Err(self.file.error_at(literal.span.start, format!("invalid hex literal `{raw}`")));
        }
        let normalized = if digits.len().is_multiple_of(2) { digits.to_string() } else { format!("0{digits}") };
        let byte_span = self.sil_span(literal.span.start, literal.span.end);
        let values = normalized
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digits = std::str::from_utf8(pair).expect("hex token is ASCII");
                u8::from_str_radix(digits, 16)
                    .map(|byte| Expr::new(ExprKind::Byte(byte), byte_span))
                    .map_err(|_| self.file.error_at(literal.span.start, format!("invalid hex literal `{raw}`")))
            })
            .collect::<Result<Vec<_>>>()?;
        self.advance();
        self.advance();
        self.advance();
        let span = self.sil_span(start, self.previous().span.end);
        if is_byte_array {
            let mut type_ref = ty.clone();
            if matches!(type_ref.array_dims[0], ArrayDim::Inferred) {
                type_ref.array_dims[0] = ArrayDim::Fixed(values.len());
            }
            return Ok(Some(Expr::new(ExprKind::Array { type_ref, values, type_span }, span)));
        }
        let expected = fixed_len.expect("non-array fixed byte sequence");
        if values.len() != expected {
            return Err(self.file.error_at(
                start,
                format!("{} hex literal size mismatch: expected {expected} bytes, got {}", ty.type_name(), values.len()),
            ));
        }
        let array = Expr::new(
            ExprKind::Array {
                type_ref: SilTypeRef { base: sil::TypeBase::Byte, array_dims: vec![ArrayDim::Fixed(expected)] },
                values,
                type_span: sil::Span::default(),
            },
            byte_span,
        );
        Ok(Some(Expr::new(ExprKind::Call { name: ty.type_name(), args: vec![array], name_span: type_span }, span)))
    }

    fn parse_typed_array(&mut self, mut ty: SilTypeRef, type_span: sil::Span<'src>, start: usize) -> Result<Expr<'src>> {
        self.expect_symbol('{')?;
        let mut values = Vec::new();
        while !self.check_symbol('}') {
            values.push(self.parse_expression()?);
            if !self.consume_symbol(',') {
                break;
            }
        }
        self.expect_symbol('}')?;
        match ty.array_dims.last_mut() {
            Some(ArrayDim::Fixed(expected)) if *expected != values.len() => {
                return Err(self
                    .file
                    .error_at(start, format!("array literal size mismatch: expected {expected}, got {}", values.len())));
            }
            Some(dim @ ArrayDim::Inferred) => *dim = ArrayDim::Fixed(values.len()),
            Some(ArrayDim::Dynamic | ArrayDim::Constant(_) | ArrayDim::Fixed(_)) => {}
            None => return Err(self.file.error_at(start, "array literal type must have a dimension")),
        }
        Ok(Expr::new(ExprKind::Array { type_ref: ty, values, type_span }, self.sil_span(start, self.previous().span.end)))
    }

    pub(super) fn parse_braced_literal(&mut self, name: String, name_span: sil::Span<'src>, start: usize) -> Result<Expr<'src>> {
        self.expect_symbol('{')?;
        let mut fields = Vec::new();
        while !self.check_symbol('}') {
            let field_start = self.current().span.start;
            let (field_name, field_name_span, expr) = if matches!(self.current().kind, TokenKind::Ident(_))
                && matches!(self.tokens.get(self.pos + 1).map(|token| &token.kind), Some(TokenKind::Symbol(':')))
            {
                let field_name = self.expect_any_ident()?;
                let field_name_span = self.sil_span(field_start, self.previous().span.end);
                self.expect_symbol(':')?;
                (field_name, field_name_span, self.parse_expression()?)
            } else {
                (String::new(), self.sil_span(field_start, field_start), self.parse_expression()?)
            };
            let span = self.sil_span(field_start, expr.span.end());
            fields.push(StateFieldExpr { name: field_name, expr, span, name_span: field_name_span });
            if !self.consume_symbol(',') {
                break;
            }
        }
        self.expect_symbol('}')?;
        Ok(Expr::new(ExprKind::StructLiteral { name, fields, name_span }, self.sil_span(start, self.previous().span.end)))
    }

    pub(super) fn parse_expression_args(&mut self) -> Result<Vec<Expr<'src>>> {
        self.expect_symbol('(')?;
        let mut args = Vec::new();
        while !self.check_symbol(')') {
            args.push(self.parse_expression()?);
            if !self.consume_symbol(',') {
                break;
            }
        }
        self.expect_symbol(')')?;
        Ok(args)
    }

    fn check_expression_postfix(&self) -> bool {
        self.check_symbol('.') || self.check_symbol('[') || self.check_ident("as")
    }

    fn parse_postfix(&mut self, source: Expr<'src>) -> Result<Expr<'src>> {
        let start = source.span.start();
        if self.consume_symbol('[') {
            let index = self.parse_expression()?;
            self.expect_symbol(']')?;
            return Ok(Expr::new(
                ExprKind::ArrayIndex { source: Box::new(source), index: Box::new(index) },
                self.sil_span(start, self.previous().span.end),
            ));
        }
        if self.consume_ident("as") {
            let type_start = self.current().span.start;
            let ty = self.parse_sil_type()?;
            let type_span = self.sil_span(type_start, self.previous().span.end);
            return Ok(Expr::new(
                ExprKind::Call { name: format!("__as_cast_{}", ty.type_name()), args: vec![source], name_span: type_span },
                self.sil_span(start, self.previous().span.end),
            ));
        }
        self.expect_symbol('.')?;
        let field_start = self.current().span.start;
        let field = match self.current().kind.clone() {
            TokenKind::Ident(field) | TokenKind::Number(field) => {
                self.advance();
                field
            }
            _ => return Err(self.error("expected field after `.`")),
        };
        let field_span = self.sil_span(field_start, self.previous().span.end);
        if let ExprKind::Split { source: split_source, index, span, .. } = &source.kind {
            let part = match field.as_str() {
                "0" => Some(SplitPart::Left),
                "1" => Some(SplitPart::Right),
                _ => None,
            };
            if let Some(part) = part {
                return Ok(Expr::new(
                    ExprKind::Split { source: split_source.clone(), index: index.clone(), part, span: *span },
                    self.sil_span(start, self.previous().span.end),
                ));
            }
            if field.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(self.file.error_at(field_start, "split() index must be 0 or 1"));
            }
        }
        if field == "length" && !self.check_symbol('(') {
            let kind = match &source.kind {
                ExprKind::FieldAccess { source: root, field: collection, .. }
                    if matches!(&root.kind, ExprKind::Identifier(root) if root == "tx") && collection == "inputs" =>
                {
                    ExprKind::Introspection(IntrospectionKind::TxInputsLength)
                }
                ExprKind::FieldAccess { source: root, field: collection, .. }
                    if matches!(&root.kind, ExprKind::Identifier(root) if root == "tx") && collection == "outputs" =>
                {
                    ExprKind::Introspection(IntrospectionKind::TxOutputsLength)
                }
                _ => ExprKind::UnarySuffix { source: Box::new(source), kind: UnarySuffixKind::Length, span: field_span },
            };
            return Ok(Expr::new(kind, self.sil_span(start, self.previous().span.end)));
        }
        if self.check_symbol('(') && self.expression_end != Some(self.pos) {
            let mut args = self.parse_expression_args()?;
            let end = self.previous().span.end;
            if field == "verify" {
                let mut parts = vec![field.as_str()];
                let mut root = &source;
                while let ExprKind::FieldAccess { source: parent, field: segment, .. } = &root.kind {
                    parts.push(segment);
                    root = parent;
                }
                if let ExprKind::Identifier(segment) = &root.kind {
                    parts.push(segment);
                    parts.reverse();
                    let name = parts.join(".");
                    if matches!(
                        name.as_str(),
                        "g16.verify"
                            | "r0.g16.verify"
                            | "r0.succinct.verify"
                            | "r0.succinct.blake2b.verify"
                            | "r0.succinct.poseidon2.verify"
                            | "r0.succinct.sha256.verify"
                    ) {
                        return Ok(Expr::new(
                            ExprKind::Call { name, args, name_span: self.sil_span(start, field_span.end()) },
                            self.sil_span(start, end),
                        ));
                    }
                }
            }
            return match field.as_str() {
                "append" if !args.is_empty() => Ok(Expr::new(
                    ExprKind::Append { source: Box::new(source), args, span: self.sil_span(field_start, end) },
                    self.sil_span(start, end),
                )),
                "split" if args.len() == 1 => Ok(Expr::new(
                    ExprKind::Split {
                        source: Box::new(source),
                        index: Box::new(args.remove(0)),
                        part: SplitPart::Left,
                        span: self.sil_span(field_start, end),
                    },
                    self.sil_span(start, end),
                )),
                "slice" if args.len() == 2 => Ok(Expr::new(
                    ExprKind::Slice {
                        source: Box::new(source),
                        start: Box::new(args.remove(0)),
                        end: Box::new(args.remove(0)),
                        span: self.sil_span(field_start, end),
                    },
                    self.sil_span(start, end),
                )),
                "co_spent" if args.is_empty() => {
                    Ok(Expr::new(ExprKind::Call { name: field, args: vec![source], name_span: field_span }, self.sil_span(start, end)))
                }
                _ => Err(self.file.error_at(field_start, format!("unsupported method call `{field}`"))),
            };
        }
        let end = self.previous().span.end;
        let kind = match (&source.kind, field.as_str()) {
            (ExprKind::Identifier(root), "activeInputIndex") if root == "this" => {
                ExprKind::Introspection(IntrospectionKind::ActiveInputIndex)
            }
            (ExprKind::Identifier(root), "activeScriptPubKey") if root == "this" => {
                ExprKind::Introspection(IntrospectionKind::ActiveScriptPubKey)
            }
            (ExprKind::Identifier(root), "bytecodeSize") if root == "this" => {
                ExprKind::Introspection(IntrospectionKind::ThisBytecodeSize)
            }
            (ExprKind::Identifier(root), "bytecodeSizeDataPrefix") if root == "this" => {
                ExprKind::Introspection(IntrospectionKind::ThisBytecodeSizeDataPrefix)
            }
            (ExprKind::Identifier(root), "version") if root == "tx" => ExprKind::Introspection(IntrospectionKind::TxVersion),
            (ExprKind::ArrayIndex { source: collection, index }, field_name) if matches!(&collection.kind, ExprKind::FieldAccess { source: root, field, .. } if matches!(&root.kind, ExprKind::Identifier(root) if root == "tx") && field == "inputs") =>
            {
                let kind = match field_name {
                    "value" => IndexedIntrospectionKind::InputValue,
                    "scriptPubKey" => IndexedIntrospectionKind::InputScriptPubKey,
                    "sigScript" => IndexedIntrospectionKind::InputSigScript,
                    "outpointTxId" => IndexedIntrospectionKind::InputOutpointTxId,
                    "outpointIndex" => IndexedIntrospectionKind::InputOutpointIndex,
                    _ => return Err(self.file.error_at(field_start, "unknown input introspection field")),
                };
                ExprKind::IndexedIntrospection { kind, index: index.clone(), field_span }
            }
            (ExprKind::ArrayIndex { source: collection, index }, field_name) if matches!(&collection.kind, ExprKind::FieldAccess { source: root, field, .. } if matches!(&root.kind, ExprKind::Identifier(root) if root == "tx") && field == "outputs") =>
            {
                let kind = match field_name {
                    "value" => IndexedIntrospectionKind::OutputValue,
                    "scriptPubKey" => IndexedIntrospectionKind::OutputScriptPubKey,
                    _ => return Err(self.file.error_at(field_start, "unknown output introspection field")),
                };
                ExprKind::IndexedIntrospection { kind, index: index.clone(), field_span }
            }
            _ => ExprKind::FieldAccess { source: Box::new(source), field, field_span },
        };
        Ok(Expr::new(kind, self.sil_span(start, end)))
    }

    fn binary_operator(&self) -> Option<(BinaryOp, u8, usize)> {
        use BinaryOp as Op;
        let next = self.tokens.get(self.pos + 1).map(|token| &token.kind);
        match (&self.current().kind, next) {
            (TokenKind::Symbol('|'), Some(TokenKind::Symbol('|'))) => Some((Op::Or, 1, 2)),
            (TokenKind::Symbol('&'), Some(TokenKind::Symbol('&'))) => Some((Op::And, 2, 2)),
            (TokenKind::Symbol('|'), _) => Some((Op::BitOr, 3, 1)),
            (TokenKind::Symbol('^'), _) => Some((Op::BitXor, 4, 1)),
            (TokenKind::Symbol('&'), _) => Some((Op::BitAnd, 5, 1)),
            (TokenKind::Symbol('='), Some(TokenKind::Symbol('='))) => Some((Op::Eq, 6, 2)),
            (TokenKind::Symbol('!'), Some(TokenKind::Symbol('='))) => Some((Op::Ne, 6, 2)),
            (TokenKind::Symbol('<'), Some(TokenKind::Symbol('='))) => Some((Op::Le, 7, 2)),
            (TokenKind::Symbol('>'), Some(TokenKind::Symbol('='))) => Some((Op::Ge, 7, 2)),
            (TokenKind::Symbol('<'), _) => Some((Op::Lt, 7, 1)),
            (TokenKind::Symbol('>'), _) => Some((Op::Gt, 7, 1)),
            (TokenKind::Symbol('+'), _) => Some((Op::Add, 8, 1)),
            (TokenKind::Symbol('-'), _) => Some((Op::Sub, 8, 1)),
            (TokenKind::Symbol('*'), _) => Some((Op::Mul, 9, 1)),
            (TokenKind::Symbol('/'), _) => Some((Op::Div, 9, 1)),
            (TokenKind::Symbol('%'), _) => Some((Op::Mod, 9, 1)),
            _ => None,
        }
    }

    fn parse_number_expression(&mut self, raw: &str, start: usize, mut end: usize) -> Result<Expr<'src>> {
        let value = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            i64::from_str_radix(hex, 16).map_err(|_| self.file.error_at(start, format!("invalid hex literal `{raw}`")))?
        } else {
            let (base, exponent) = raw.split_once(['e', 'E']).map_or((raw, None), |(base, exponent)| (base, Some(exponent)));
            let base = base.replace('_', "");
            let mut value = base.parse::<i128>().map_err(|_| self.file.error_at(start, format!("invalid number literal `{raw}`")))?;
            if let Some(exponent) = exponent {
                let exponent = exponent.replace('_', "").parse::<u32>().map_err(|_| self.file.error_at(start, "invalid exponent"))?;
                let scale = 10_i128.checked_pow(exponent).ok_or_else(|| self.file.error_at(start, "number literal overflow"))?;
                value = value.checked_mul(scale).ok_or_else(|| self.file.error_at(start, "number literal overflow"))?;
            }
            i64::try_from(value).map_err(|_| self.file.error_at(start, "number literal overflow"))?
        };
        let unit = match &self.current().kind {
            TokenKind::Ident(unit) => match unit.as_str() {
                "seconds" | "minutes" | "hours" | "days" | "weeks" | "litras" | "grains" | "kas" => Some(unit.clone()),
                _ => None,
            },
            _ => None,
        };
        let kind = if let Some(unit) = unit {
            end = self.current().span.end;
            self.advance();
            let (scale, temporal) = match unit.as_str() {
                "seconds" => (1_000, true),
                "minutes" => (60_000, true),
                "hours" => (3_600_000, true),
                "days" => (86_400_000, true),
                "weeks" => (604_800_000, true),
                "litras" => (1, false),
                "grains" => (100_000, false),
                "kas" => (100_000_000, false),
                _ => unreachable!("unit was matched above"),
            };
            let scaled = value.checked_mul(scale).ok_or_else(|| self.file.error_at(start, "number unit overflow"))?;
            if temporal { ExprKind::Temporal(scaled) } else { ExprKind::Int(scaled) }
        } else {
            ExprKind::Int(value)
        };
        Ok(Expr::new(kind, self.sil_span(start, end)))
    }

    pub(super) fn sil_span(&self, start: usize, end: usize) -> sil::Span<'src> {
        sil::Span::new(self.source, start, end).expect("token spans lie within their immutable source")
    }
}
