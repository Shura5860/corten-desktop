//! Safe inline arithmetic evaluator for Corten Desktop Launcher.

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    Pow,
    LParen,
    RParen,
}

struct MathParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl MathParser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Option<f64> {
        let mut left = self.parse_term()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Plus => {
                    self.next();
                    let right = self.parse_term()?;
                    left += right;
                }
                Token::Minus => {
                    self.next();
                    let right = self.parse_term()?;
                    left -= right;
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_term(&mut self) -> Option<f64> {
        let mut left = self.parse_power()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Mul => {
                    self.next();
                    let right = self.parse_power()?;
                    left *= right;
                }
                Token::Div => {
                    self.next();
                    let right = self.parse_power()?;
                    if right.abs() < 1e-12 {
                        return None; // Zero division
                    }
                    left /= right;
                }
                Token::Mod => {
                    self.next();
                    let right = self.parse_power()?;
                    if right.abs() < 1e-12 {
                        return None;
                    }
                    left %= right;
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_power(&mut self) -> Option<f64> {
        let left = self.parse_factor()?;
        if let Some(Token::Pow) = self.peek() {
            self.next();
            let right = self.parse_power()?;
            Some(left.powf(right))
        } else {
            Some(left)
        }
    }

    fn parse_factor(&mut self) -> Option<f64> {
        match self.next()? {
            Token::Num(n) => Some(n),
            Token::Plus => self.parse_factor(),
            Token::Minus => Some(-self.parse_factor()?),
            Token::LParen => {
                let inner = self.parse_expr()?;
                if let Some(Token::RParen) = self.next() {
                    Some(inner)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Evaluates a user input expression if it is a valid arithmetic expression.
/// Returns None if the input contains non-math symbols or fails to parse.
pub fn eval_math_expression(input: &str) -> Option<f64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }

    // Must have at least one digit and at least one operator
    let has_operator = s.chars().any(|c| matches!(c, '+' | '-' | '*' | '/' | '%' | '^'));
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    if !has_operator || !has_digit {
        return None;
    }

    // Strictly whitelist characters
    if !s.chars().all(|c| {
        c.is_ascii_digit()
            || c.is_whitespace()
            || matches!(c, '.' | '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')')
    }) {
        return None;
    }

    // Tokenize
    let mut tokens = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let mut num_str = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                num_str.push(chars[i]);
                i += 1;
            }
            let val = num_str.parse::<f64>().ok()?;
            tokens.push(Token::Num(val));
        } else {
            match c {
                '+' => tokens.push(Token::Plus),
                '-' => tokens.push(Token::Minus),
                '*' => tokens.push(Token::Mul),
                '/' => tokens.push(Token::Div),
                '%' => tokens.push(Token::Mod),
                '^' => tokens.push(Token::Pow),
                '(' => tokens.push(Token::LParen),
                ')' => tokens.push(Token::RParen),
                _ => return None,
            }
            i += 1;
        }
    }

    let mut parser = MathParser { tokens, pos: 0 };
    let res = parser.parse_expr()?;
    if parser.pos == parser.tokens.len() && res.is_finite() {
        Some(res)
    } else {
        None
    }
}

/// Formats a float calculation result into a clean human-readable string.
pub fn format_calc_result(val: f64) -> String {
    if val.is_infinite() || val.is_nan() {
        return "Tanımsız".to_string();
    }
    if (val.fract()).abs() < 1e-9 {
        format!("{:.0}", val)
    } else {
        let s = format!("{:.6}", val);
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    }
}

/// Parses hex/rgb color strings and returns a formatted color swatch with conversions.
pub fn parse_color_query(q: &str) -> Option<(String, String)> {
    let s = q.trim();
    if s.starts_with('#') {
        let hex = s.trim_start_matches('#');
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let (h, sat, l) = rgb_to_hsl(r, g, b);
            return Some((
                format!("🎨 Renk: #{}", hex.to_uppercase()),
                format!("RGB: ({}, {}, {}) | HSL: ({}°, {}%, {}%)", r, g, b, h, sat, l),
            ));
        } else if hex.len() == 3 {
            let r_ch = hex.chars().next()?;
            let g_ch = hex.chars().nth(1)?;
            let b_ch = hex.chars().nth(2)?;
            let full_hex = format!("{}{}{}{}{}{}", r_ch, r_ch, g_ch, g_ch, b_ch, b_ch);
            let r = u8::from_str_radix(&full_hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&full_hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&full_hex[4..6], 16).ok()?;
            let (h, sat, l) = rgb_to_hsl(r, g, b);
            return Some((
                format!("🎨 Renk: #{}", full_hex.to_uppercase()),
                format!("RGB: ({}, {}, {}) | HSL: ({}°, {}%, {}%)", r, g, b, h, sat, l),
            ));
        }
    } else if s.starts_with("rgb(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() == 3 {
            let r = parts[0].parse::<u8>().ok()?;
            let g = parts[1].parse::<u8>().ok()?;
            let b = parts[2].parse::<u8>().ok()?;
            let (h, sat, l) = rgb_to_hsl(r, g, b);
            return Some((
                format!("🎨 Renk: #{:02X}{:02X}{:02X}", r, g, b),
                format!("RGB: ({}, {}, {}) | HSL: ({}°, {}%, {}%)", r, g, b, h, sat, l),
            ));
        }
    }
    None
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u16, u8, u8) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf.max(bf));
    let min = rf.min(gf.min(bf));
    let delta = max - min;
    let l = (max + min) / 2.0;

    let s = if delta == 0.0 {
        0.0
    } else if l < 0.5 {
        delta / (max + min)
    } else {
        delta / (2.0 - max - min)
    };

    let h = if delta == 0.0 {
        0.0
    } else if (max - rf).abs() < f32::EPSILON {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if (max - gf).abs() < f32::EPSILON {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    let h_deg = ((h + 360.0) % 360.0).round() as u16;
    let s_pct = (s * 100.0).round() as u8;
    let l_pct = (l * 100.0).round() as u8;
    (h_deg, s_pct, l_pct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_parser() {
        assert!(parse_color_query("#ff7300").is_some());
        assert!(parse_color_query("#fff").is_some());
        assert!(parse_color_query("rgb(255, 115, 0)").is_some());
        assert!(parse_color_query("notacolor").is_none());
    }

    #[test]
    fn test_arithmetic_evaluator() {
        assert_eq!(eval_math_expression("2 + 2"), Some(4.0));
        assert_eq!(eval_math_expression("10 - 3 * 2"), Some(4.0));
        assert_eq!(eval_math_expression("(10 - 3) * 2"), Some(14.0));
        assert_eq!(eval_math_expression("2 ^ 3"), Some(8.0));
        assert_eq!(eval_math_expression("15 / 3"), Some(5.0));
        assert_eq!(eval_math_expression("10 % 3"), Some(1.0));
        assert_eq!(eval_math_expression("-4 + 10"), Some(6.0));
        assert_eq!(eval_math_expression("firefox"), None);
        assert_eq!(eval_math_expression("10 / 0"), None);
    }

    #[test]
    fn test_format_calc_result() {
        assert_eq!(format_calc_result(4.0), "4");
        assert_eq!(format_calc_result(2.5), "2.5");
        assert_eq!(format_calc_result(100.125), "100.125");
    }
}

