use std::fmt;

use mago_fingerprint::{FingerprintOptions, Fingerprintable};
use mago_names::ResolvedNames;
use mago_span::HasSpan;
use mago_syntax::cst::{ArrayElement, BinaryOperator, Expression, Literal, UnaryPrefixOperator};

/// A PHP array key after PHP's scalar-key coercions have been applied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayKey {
    Integer(i128),
    String(Vec<u8>),
}

/// An expression which Mago parsed but which the checker cannot evaluate safely.
///
/// Equality is based on Mago's position-independent, name-resolved fingerprint. The
/// source spelling is retained solely for diagnostics.
#[derive(Debug, Clone)]
pub struct OpaqueExpression {
    pub source: String,
    pub fingerprint: u64,
}

impl PartialEq for OpaqueExpression {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}

impl Eq for OpaqueExpression {}

/// A value used by a PHP declaration.
///
/// `Opaque` is deliberately not guessed at. Comparing two different opaque
/// expressions should be reported as a skipped check by the comparison layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhpValue {
    Null,
    Bool(bool),
    Integer(i128),
    Float(u64),
    String(Vec<u8>),
    Array(Vec<(ArrayKey, PhpValue)>),
    Opaque(OpaqueExpression),
}

impl PhpValue {
    #[must_use]
    pub fn is_supported(&self) -> bool {
        match self {
            Self::Array(entries) => {
                let mut index = 0;
                while index < entries.len() {
                    if !entries[index].1.is_supported() {
                        return false;
                    }
                    index += 1;
                }
                true
            }
            Self::Opaque(_) => false,
            _ => true,
        }
    }

    /// Match PHP's strict value comparison for the value subset evaluated here.
    #[must_use]
    pub fn strictly_equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Integer(left), Self::Integer(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => f64::from_bits(*left) == f64::from_bits(*right),
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Array(left), Self::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|((left_key, left_value), (right_key, right_value))| {
                            left_key == right_key && left_value.strictly_equals(right_value)
                        })
            }
            (Self::Opaque(left), Self::Opaque(right)) => left == right,
            _ => false,
        }
    }

    /// Render a value using the subset of `var_export()` used in change messages.
    #[must_use]
    pub fn render_php(&self) -> String {
        let mut output = String::new();
        self.render_into(&mut output, 0);
        output
    }

    fn render_into(&self, output: &mut String, indent: usize) {
        match self {
            Self::Null => output.push_str("NULL"),
            Self::Bool(true) => output.push_str("true"),
            Self::Bool(false) => output.push_str("false"),
            Self::Integer(value) => output.push_str(&value.to_string()),
            Self::Float(bits) => {
                let value = f64::from_bits(*bits);
                if value.is_nan() {
                    output.push_str("NAN");
                } else if value == f64::INFINITY {
                    output.push_str("INF");
                } else if value == f64::NEG_INFINITY {
                    output.push_str("-INF");
                } else {
                    output.push_str(&value.to_string());
                    if value.fract() == 0.0 {
                        output.push_str(".0");
                    }
                }
            }
            Self::String(value) => {
                output.push('\'');
                for character in String::from_utf8_lossy(value).chars() {
                    match character {
                        '\\' => output.push_str("\\\\"),
                        '\'' => output.push_str("\\'"),
                        _ => output.push(character),
                    }
                }
                output.push('\'');
            }
            Self::Array(entries) => {
                output.push_str("array (\n");
                for (key, value) in entries {
                    output.push_str(&" ".repeat(indent + 2));
                    match key {
                        ArrayKey::Integer(value) => output.push_str(&value.to_string()),
                        ArrayKey::String(value) => PhpValue::String(value.clone()).render_into(output, indent + 2),
                    }
                    output.push_str(" => ");
                    value.render_into(output, indent + 2);
                    output.push_str(",\n");
                }
                output.push_str(&" ".repeat(indent));
                output.push(')');
            }
            Self::Opaque(expression) => output.push_str(&expression.source),
        }
    }
}

impl fmt::Display for PhpValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render_php())
    }
}

pub(crate) fn value_from_expression(
    expression: &Expression<'_>,
    source: &[u8],
    resolved_names: &ResolvedNames<'_>,
) -> PhpValue {
    evaluate(expression, source, resolved_names)
        .unwrap_or_else(|| opaque_expression(expression, source, resolved_names))
}

fn evaluate(expression: &Expression<'_>, source: &[u8], resolved_names: &ResolvedNames<'_>) -> Option<PhpValue> {
    match expression {
        Expression::Literal(literal) => match literal {
            Literal::String(string) => string.value.map(|value| PhpValue::String(value.to_vec())),
            Literal::Integer(integer) => integer.value.map(|value| PhpValue::Integer(i128::from(value))),
            Literal::Float(float) => Some(PhpValue::Float(float.value.into_inner().to_bits())),
            Literal::True(_) => Some(PhpValue::Bool(true)),
            Literal::False(_) => Some(PhpValue::Bool(false)),
            Literal::Null(_) => Some(PhpValue::Null),
        },
        Expression::Parenthesized(parenthesized) => evaluate(parenthesized.expression, source, resolved_names),
        Expression::UnaryPrefix(unary) => {
            let operand = evaluate(unary.operand, source, resolved_names)?;
            evaluate_unary(&unary.operator, operand)
        }
        Expression::Binary(binary) => {
            let left = evaluate(binary.lhs, source, resolved_names)?;
            let right = evaluate(binary.rhs, source, resolved_names)?;
            evaluate_binary(&binary.operator, left, right)
        }
        Expression::Array(array) => evaluate_array(array.elements.iter(), source, resolved_names),
        Expression::LegacyArray(array) => evaluate_array(array.elements.iter(), source, resolved_names),
        _ => None,
    }
}

fn evaluate_array<'ast, 'arena>(
    elements: impl Iterator<Item = &'ast ArrayElement<'arena>>,
    source: &[u8],
    resolved_names: &ResolvedNames<'_>,
) -> Option<PhpValue>
where
    'arena: 'ast,
{
    let mut entries = Vec::new();
    let mut next_integer_key = 0i128;

    for element in elements {
        let (key, value) = match element {
            ArrayElement::KeyValue(element) => {
                let key = evaluate(element.key, source, resolved_names)?;
                let key = coerce_array_key(key)?;
                let value = evaluate(element.value, source, resolved_names)?;
                (key, value)
            }
            ArrayElement::Value(element) => {
                let key = ArrayKey::Integer(next_integer_key);
                let value = evaluate(element.value, source, resolved_names)?;
                (key, value)
            }
            ArrayElement::Variadic(_) | ArrayElement::Missing(_) => return None,
        };

        if let ArrayKey::Integer(key_value) = key {
            next_integer_key = next_integer_key.max(key_value.saturating_add(1));
            entries.push((ArrayKey::Integer(key_value), value));
        } else {
            entries.push((key, value));
        }
    }

    Some(PhpValue::Array(entries))
}

fn coerce_array_key(value: PhpValue) -> Option<ArrayKey> {
    match value {
        PhpValue::Integer(value) => Some(ArrayKey::Integer(value)),
        PhpValue::String(value) => {
            parse_integer_array_key(&value).map_or(Some(ArrayKey::String(value)), |key| Some(ArrayKey::Integer(key)))
        }
        PhpValue::Bool(value) => Some(ArrayKey::Integer(i128::from(value))),
        PhpValue::Null => Some(ArrayKey::String(Vec::new())),
        PhpValue::Float(bits) => Some(ArrayKey::Integer(f64::from_bits(bits) as i128)),
        PhpValue::Array(_) | PhpValue::Opaque(_) => None,
    }
}

fn parse_integer_array_key(value: &[u8]) -> Option<i128> {
    let text = std::str::from_utf8(value).ok()?;
    if text.starts_with('+') || (text.len() > 1 && text.starts_with('0')) || text == "-0" {
        return None;
    }
    text.parse().ok()
}

fn evaluate_unary(operator: &UnaryPrefixOperator<'_>, operand: PhpValue) -> Option<PhpValue> {
    match (operator, operand) {
        (UnaryPrefixOperator::Plus(_), PhpValue::Integer(value)) => Some(PhpValue::Integer(value)),
        (UnaryPrefixOperator::Plus(_), PhpValue::Float(value)) => Some(PhpValue::Float(value)),
        (UnaryPrefixOperator::Negation(_), PhpValue::Integer(value)) => Some(PhpValue::Integer(value.saturating_neg())),
        (UnaryPrefixOperator::Negation(_), PhpValue::Float(value)) => {
            Some(PhpValue::Float((-f64::from_bits(value)).to_bits()))
        }
        (UnaryPrefixOperator::BitwiseNot(_), PhpValue::Integer(value)) => Some(PhpValue::Integer(!value)),
        (UnaryPrefixOperator::Not(_), PhpValue::Bool(value)) => Some(PhpValue::Bool(!value)),
        _ => None,
    }
}

fn evaluate_binary(operator: &BinaryOperator<'_>, left: PhpValue, right: PhpValue) -> Option<PhpValue> {
    match (operator, left, right) {
        (BinaryOperator::Addition(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left.saturating_add(right)))
        }
        (BinaryOperator::Subtraction(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left.saturating_sub(right)))
        }
        (BinaryOperator::Multiplication(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left.saturating_mul(right)))
        }
        (BinaryOperator::Division(_), PhpValue::Integer(left), PhpValue::Integer(right)) if right != 0 => {
            if left % right == 0 {
                Some(PhpValue::Integer(left / right))
            } else {
                Some(PhpValue::Float(((left as f64) / (right as f64)).to_bits()))
            }
        }
        (BinaryOperator::Modulo(_), PhpValue::Integer(left), PhpValue::Integer(right)) if right != 0 => {
            Some(PhpValue::Integer(left % right))
        }
        (BinaryOperator::Exponentiation(_), PhpValue::Integer(left), PhpValue::Integer(right))
            if (0..=u32::MAX as i128).contains(&right) =>
        {
            Some(PhpValue::Integer(left.saturating_pow(right as u32)))
        }
        (BinaryOperator::BitwiseAnd(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left & right))
        }
        (BinaryOperator::BitwiseOr(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left | right))
        }
        (BinaryOperator::BitwiseXor(_), PhpValue::Integer(left), PhpValue::Integer(right)) => {
            Some(PhpValue::Integer(left ^ right))
        }
        (BinaryOperator::LeftShift(_), PhpValue::Integer(left), PhpValue::Integer(right))
            if (0..128).contains(&right) =>
        {
            Some(PhpValue::Integer(left.wrapping_shl(right as u32)))
        }
        (BinaryOperator::RightShift(_), PhpValue::Integer(left), PhpValue::Integer(right))
            if (0..128).contains(&right) =>
        {
            Some(PhpValue::Integer(left.wrapping_shr(right as u32)))
        }
        (BinaryOperator::StringConcat(_), PhpValue::String(mut left), PhpValue::String(right)) => {
            left.extend(right);
            Some(PhpValue::String(left))
        }
        _ => None,
    }
}

fn opaque_expression(expression: &Expression<'_>, source: &[u8], resolved_names: &ResolvedNames<'_>) -> PhpValue {
    let span = expression.span();
    let expression_source = source
        .get(span.start.offset as usize..span.end.offset as usize)
        .map(String::from_utf8_lossy)
        .map_or_else(|| "<unsupported expression>".to_owned(), |source| source.into_owned());
    let fingerprint = expression.fingerprint(resolved_names, &FingerprintOptions::default());

    PhpValue::Opaque(OpaqueExpression {
        source: expression_source,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_scalar_values_like_php() {
        assert_eq!(PhpValue::Null.render_php(), "NULL");
        assert_eq!(PhpValue::Bool(true).render_php(), "true");
        assert_eq!(
            PhpValue::String(b"it's \\ fine".to_vec()).render_php(),
            "'it\\'s \\\\ fine'"
        );
        assert_eq!(PhpValue::String("café".as_bytes().to_vec()).render_php(), "'café'");
        assert!(PhpValue::Float((-0.0f64).to_bits()).strictly_equals(&PhpValue::Float(0.0f64.to_bits())));
        assert!(!PhpValue::Float(f64::NAN.to_bits()).strictly_equals(&PhpValue::Float(f64::NAN.to_bits())));
    }

    #[test]
    fn unsupported_expressions_compare_by_fingerprint() {
        let first = PhpValue::Opaque(OpaqueExpression {
            source: "A::B".to_owned(),
            fingerprint: 7,
        });
        let second = PhpValue::Opaque(OpaqueExpression {
            source: "Alias::B".to_owned(),
            fingerprint: 7,
        });
        assert_eq!(first, second);
        assert!(!first.is_supported());
    }
}
