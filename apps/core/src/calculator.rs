use std::f64;

#[derive(Debug)]
pub(crate) enum CalcError {
    Empty,
    Syntax(String),
    Math(String),
}

impl std::fmt::Display for CalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty expression"),
            Self::Syntax(msg) => write!(f, "{msg}"),
            Self::Math(msg) => write!(f, "{msg}"),
        }
    }
}

pub(crate) fn evaluate(input: &str) -> Result<f64, CalcError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CalcError::Empty);
    }
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let result = parse_expr(&chars, &mut pos)?;
    skip_whitespace(&chars, &mut pos);
    if pos < chars.len() {
        return Err(CalcError::Syntax(format!(
            "unexpected character '{}' at position {}",
            chars[pos], pos
        )));
    }
    Ok(result)
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

fn parse_expr(chars: &[char], pos: &mut usize) -> Result<f64, CalcError> {
    let mut left = parse_term(chars, pos)?;
    loop {
        skip_whitespace(chars, pos);
        if *pos >= chars.len() {
            break;
        }
        match chars[*pos] {
            '+' => {
                *pos += 1;
                let right = parse_term(chars, pos)?;
                left += right;
            }
            '-' => {
                *pos += 1;
                let right = parse_term(chars, pos)?;
                left -= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_term(chars: &[char], pos: &mut usize) -> Result<f64, CalcError> {
    let mut left = parse_unary(chars, pos)?;
    loop {
        skip_whitespace(chars, pos);
        if *pos >= chars.len() {
            break;
        }
        match chars[*pos] {
            '*' => {
                *pos += 1;
                let right = parse_unary(chars, pos)?;
                left *= right;
            }
            '/' => {
                *pos += 1;
                let right = parse_unary(chars, pos)?;
                if right == 0.0 {
                    return Err(CalcError::Math("division by zero".into()));
                }
                left /= right;
            }
            '%' => {
                *pos += 1;
                let right = parse_unary(chars, pos)?;
                if right == 0.0 {
                    return Err(CalcError::Math("modulo by zero".into()));
                }
                left %= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_unary(chars: &[char], pos: &mut usize) -> Result<f64, CalcError> {
    skip_whitespace(chars, pos);
    if *pos >= chars.len() {
        return Err(CalcError::Syntax("unexpected end of expression".into()));
    }
    match chars[*pos] {
        '+' => {
            *pos += 1;
            parse_unary(chars, pos)
        }
        '-' => {
            *pos += 1;
            let val = parse_unary(chars, pos)?;
            Ok(-val)
        }
        _ => {
            let mut left = parse_power(chars, pos)?;
            skip_whitespace(chars, pos);
            if *pos < chars.len() && chars[*pos] == '^' {
                *pos += 1;
                let right = parse_unary(chars, pos)?;
                left = left.powf(right);
            }
            Ok(left)
        }
    }
}

fn parse_power(chars: &[char], pos: &mut usize) -> Result<f64, CalcError> {
    parse_primary(chars, pos)
}

fn parse_primary(chars: &[char], pos: &mut usize) -> Result<f64, CalcError> {
    skip_whitespace(chars, pos);
    if *pos >= chars.len() {
        return Err(CalcError::Syntax("unexpected end of expression".into()));
    }

    if chars[*pos] == '(' {
        *pos += 1;
        let val = parse_expr(chars, pos)?;
        skip_whitespace(chars, pos);
        if *pos >= chars.len() || chars[*pos] != ')' {
            return Err(CalcError::Syntax("missing closing parenthesis".into()));
        }
        *pos += 1;
        return Ok(val);
    }

    // Parse constants and function names (sqrt, sin, cos, tan, abs, floor, ceil, round, ln, log, exp)
    if chars[*pos].is_ascii_alphabetic() || chars[*pos] == '_' {
        let start = *pos;
        while *pos < chars.len() && (chars[*pos].is_ascii_alphanumeric() || chars[*pos] == '_') {
            *pos += 1;
        }
        let name: String = chars[start..*pos].iter().collect();
        let name_lower = name.to_ascii_lowercase();

        // Check for constants first (no parentheses)
        match name_lower.as_str() {
            "pi" => return Ok(std::f64::consts::PI),
            "e" => return Ok(std::f64::consts::E),
            "tau" => return Ok(std::f64::consts::TAU),
            "inf" | "infinity" => return Ok(f64::INFINITY),
            "nan" => return Ok(f64::NAN),
            _ => {}
        }

        skip_whitespace(chars, pos);
        if *pos >= chars.len() || chars[*pos] != '(' {
            return Err(CalcError::Syntax(format!("expected '(' after '{name}'")));
        }
        *pos += 1;
        let arg = parse_expr(chars, pos)?;
        skip_whitespace(chars, pos);
        if *pos >= chars.len() || chars[*pos] != ')' {
            return Err(CalcError::Syntax(format!(
                "missing ')' after function '{name}'"
            )));
        }
        *pos += 1;

        return match name_lower.as_str() {
            "sqrt" => Ok(arg.sqrt()),
            "cbrt" => Ok(arg.cbrt()),
            "sin" => Ok(arg.sin()),
            "cos" => Ok(arg.cos()),
            "tan" => Ok(arg.tan()),
            "asin" => Ok(arg.asin()),
            "acos" => Ok(arg.acos()),
            "atan" => Ok(arg.atan()),
            "abs" => Ok(arg.abs()),
            "floor" => Ok(arg.floor()),
            "ceil" => Ok(arg.ceil()),
            "round" => Ok(arg.round()),
            "ln" | "log" => {
                if arg <= 0.0 {
                    Err(CalcError::Math("logarithm of non-positive number".into()))
                } else {
                    Ok(arg.ln())
                }
            }
            "log10" => {
                if arg <= 0.0 {
                    Err(CalcError::Math("logarithm of non-positive number".into()))
                } else {
                    Ok(arg.log10())
                }
            }
            "exp" => Ok(arg.exp()),
            "sign" => Ok(if arg > 0.0 {
                1.0
            } else if arg < 0.0 {
                -1.0
            } else {
                0.0
            }),
            _ => Err(CalcError::Syntax(format!("unknown function '{name}'"))),
        };
    }

    // Parse number
    let start = *pos;
    if chars[*pos] == '.' || chars[*pos].is_ascii_digit() {
        let mut has_dot = false;
        while *pos < chars.len() {
            if chars[*pos] == '.' {
                if has_dot {
                    break;
                }
                has_dot = true;
            } else if !chars[*pos].is_ascii_digit() {
                break;
            }
            *pos += 1;
        }
        let num_str: String = chars[start..*pos].iter().collect();
        num_str
            .parse::<f64>()
            .map_err(|_| CalcError::Syntax(format!("invalid number '{num_str}'")))
    } else {
        Err(CalcError::Syntax(format!(
            "unexpected character '{}'",
            chars[*pos]
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_arithmetic() {
        assert_eq!(evaluate("2+3").unwrap(), 5.0);
        assert_eq!(evaluate("10-4").unwrap(), 6.0);
        assert_eq!(evaluate("3*4").unwrap(), 12.0);
        assert_eq!(evaluate("15/3").unwrap(), 5.0);
    }

    #[test]
    fn operator_precedence() {
        assert_eq!(evaluate("2+3*4").unwrap(), 14.0);
        assert_eq!(evaluate("10-6/2").unwrap(), 7.0);
        assert_eq!(evaluate("2*3+4*5").unwrap(), 26.0);
    }

    #[test]
    fn parentheses() {
        assert_eq!(evaluate("(2+3)*4").unwrap(), 20.0);
        assert_eq!(evaluate("2*(3+4)").unwrap(), 14.0);
        assert_eq!(evaluate("(2+3)*(4+5)").unwrap(), 45.0);
    }

    #[test]
    fn unary_minus() {
        assert_eq!(evaluate("-5+3").unwrap(), -2.0);
        assert_eq!(evaluate("10+-3").unwrap(), 7.0);
        assert_eq!(evaluate("-(5+3)").unwrap(), -8.0);
    }

    #[test]
    fn power() {
        assert_eq!(evaluate("2^3").unwrap(), 8.0);
        assert_eq!(evaluate("3^2^2").unwrap(), 81.0); // right-associative
    }

    #[test]
    fn modulo() {
        assert_eq!(evaluate("10%3").unwrap(), 1.0);
        assert_eq!(evaluate("7%2").unwrap(), 1.0);
    }

    #[test]
    fn whitespace_handling() {
        assert_eq!(evaluate("  2  +  3  ").unwrap(), 5.0);
        assert_eq!(evaluate("( 2 + 3 )").unwrap(), 5.0);
    }

    #[test]
    fn division_by_zero() {
        assert!(evaluate("5/0").is_err());
    }

    #[test]
    fn unmatched_parenthesis() {
        assert!(evaluate("(2+3").is_err());
    }

    #[test]
    fn empty_expression() {
        assert!(evaluate("").is_err());
        assert!(evaluate("   ").is_err());
    }

    #[test]
    fn functions() {
        assert_eq!(evaluate("sqrt(9)").unwrap(), 3.0);
        assert_eq!(evaluate("abs(-5)").unwrap(), 5.0);
        assert!(evaluate("sqrt(-1)").unwrap().is_nan());
        assert!(evaluate("ln(0)").is_err());
        assert_eq!(evaluate("round(3.7)").unwrap(), 4.0);
        assert_eq!(evaluate("floor(3.7)").unwrap(), 3.0);
        assert_eq!(evaluate("ceil(3.2)").unwrap(), 4.0);
    }

    #[test]
    fn decimals() {
        assert!((evaluate("3.14*2").unwrap() - 6.28).abs() < 1e-10);
        assert!((evaluate("0.5+0.25").unwrap() - 0.75).abs() < 1e-10);
    }

    #[test]
    fn pi_and_e_constants() {
        assert!((evaluate("pi").unwrap() - std::f64::consts::PI).abs() < 1e-10);
        assert!((evaluate("e").unwrap() - std::f64::consts::E).abs() < 1e-10);
        assert!((evaluate("2*pi").unwrap() - 2.0 * std::f64::consts::PI).abs() < 1e-10);
    }

    #[test]
    fn complex_expression() {
        let result = evaluate("2*(3+4)^2-5/2").unwrap();
        assert!((result - 95.5).abs() < 1e-10);
    }

    #[test]
    fn error_messages() {
        let err = evaluate("2++").unwrap_err();
        assert!(err.to_string().contains("unexpected end"));
    }
}
