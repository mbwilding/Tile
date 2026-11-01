//! The launcher's calculator: arithmetic with `+ - * / % ^` (`x` multiplies too), brackets,
//! functions (`sqrt`, `sin`, `ln`, ...), hex numbers (`0x1f`) and the constants `pi`, `tau`
//! and `e`.

use std::f64::consts::{E, PI, TAU};

/// The value of `text` if it reads as a calculation: it has a digit, and is more than a bare
/// number.
pub fn calculation(text: &str) -> Option<f64> {
    let text = text.trim();
    if !text.chars().any(|c| c.is_ascii_digit()) || text.parse::<f64>().is_ok() {
        return None;
    }
    evaluate(text)
}

/// The value of the expression `text`, or `None` if it isn't one or has no finite value.
pub fn evaluate(text: &str) -> Option<f64> {
    let mut parser = Parser {
        chars: text.chars().filter(|c| !c.is_whitespace()).collect(),
        at: 0,
    };
    let value = parser.sum()?;
    (parser.at == parser.chars.len() && value.is_finite()).then_some(value)
}

/// `value` written out: whole numbers without decimals, others to 10 places without trailing
/// zeros, and very large or small ones in exponent form.
pub fn format(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    let size = value.abs();
    if size != 0.0 && !(1e-6..1e15).contains(&size) {
        return format!("{value:e}");
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    let fixed = format!("{value:.10}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.at += 1;
        }
        found
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        loop {
            if self.eat('+') {
                value += self.product()?;
            } else if self.eat('-') || self.eat('−') {
                value -= self.product()?;
            } else {
                return Some(value);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            if self.peek() == Some('*') && self.chars.get(self.at + 1) != Some(&'*') {
                self.at += 1;
                value *= self.unary()?;
            } else if self.eat('×') || self.eat('x') || self.eat('X') {
                value *= self.unary()?;
            } else if self.eat('/') || self.eat('÷') {
                value /= self.unary()?;
            } else if self.eat('%') {
                value %= self.unary()?;
            } else {
                return Some(value);
            }
        }
    }

    /// A sign binds looser than `^`, so `-2^2` is -4.
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') || self.eat('−') {
            return self.unary().map(|v| -v);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        let raised = if self.eat('^') {
            true
        } else if self.peek() == Some('*') && self.chars.get(self.at + 1) == Some(&'*') {
            self.at += 2;
            true
        } else {
            false
        };
        if raised {
            Some(base.powf(self.unary()?))
        } else {
            Some(base)
        }
    }

    fn primary(&mut self) -> Option<f64> {
        if self.eat('(') {
            let value = self.sum()?;
            return self.eat(')').then_some(value);
        }
        match self.peek()? {
            c if c.is_ascii_digit() || c == '.' => self.number(),
            c if c.is_ascii_alphabetic() => self.name(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<f64> {
        let start = self.at;
        // `0x` starts a hex number rather than multiplying by zero.
        if self.peek() == Some('0')
            && matches!(self.chars.get(self.at + 1), Some('x' | 'X'))
            && self
                .chars
                .get(self.at + 2)
                .is_some_and(char::is_ascii_hexdigit)
        {
            self.at += 2;
            let digits_start = self.at;
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_hexdigit() || c == '_')
            {
                self.at += 1;
            }
            let digits: String = self.chars[digits_start..self.at]
                .iter()
                .filter(|&&c| c != '_')
                .collect();
            return u64::from_str_radix(&digits, 16).ok().map(|n| n as f64);
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '_')
        {
            self.at += 1;
        }
        // An exponent, but not the constant `e` after a number.
        if matches!(self.peek(), Some('e' | 'E')) {
            let next = self.chars.get(self.at + 1).copied();
            let after = self.chars.get(self.at + 2).copied();
            let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
            if digit(next) || (matches!(next, Some('+' | '-')) && digit(after)) {
                self.at += 2;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.at += 1;
                }
            }
        }
        let text: String = self.chars[start..self.at]
            .iter()
            .filter(|&&c| c != '_')
            .collect();
        text.parse().ok()
    }

    fn name(&mut self) -> Option<f64> {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.at += 1;
        }
        let name: String = self.chars[start..self.at].iter().collect();
        let name = name.to_ascii_lowercase();
        let constant = match name.as_str() {
            "pi" => Some(PI),
            "tau" => Some(TAU),
            "e" => Some(E),
            _ => None,
        };
        if constant.is_some() {
            return constant;
        }
        let f: fn(f64) -> f64 = match name.as_str() {
            "sqrt" => f64::sqrt,
            "cbrt" => f64::cbrt,
            "abs" => f64::abs,
            "ln" => f64::ln,
            "log" => f64::log10,
            "log2" => f64::log2,
            "exp" => f64::exp,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            "asin" => f64::asin,
            "acos" => f64::acos,
            "atan" => f64::atan,
            "floor" => f64::floor,
            "ceil" => f64::ceil,
            "round" => f64::round,
            _ => return None,
        };
        if !self.eat('(') {
            return None;
        }
        let arg = self.sum()?;
        self.eat(')').then(|| f(arg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(text: &str) -> f64 {
        evaluate(text).unwrap_or_else(|| panic!("`{text}` didn't evaluate"))
    }

    #[test]
    fn follows_precedence() {
        assert_eq!(eval("1 + 2 * 3"), 7.0);
        assert_eq!(eval("(1 + 2) * 3"), 9.0);
        assert_eq!(eval("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(eval("2 ** 10"), 1024.0);
        assert_eq!(eval("-2^2"), -4.0);
        assert_eq!(eval("10 % 4"), 2.0);
        assert_eq!(eval("8 / 2 / 2"), 2.0);
        assert_eq!(eval("3 × 4 ÷ 2"), 6.0);
        assert_eq!(eval("3 x 4"), 12.0);
        assert_eq!(eval("2X3x4"), 24.0);
        assert_eq!(eval("1 + 2 x 3"), 7.0);
        assert_eq!(eval("2 x 3 ^ 2"), 18.0);
    }

    #[test]
    fn x_is_not_taken_from_names_or_hex() {
        assert_eq!(eval("0x1f"), 31.0);
        assert_eq!(eval("0xff + 1"), 256.0);
        assert_eq!(eval("2 x 0x10"), 32.0);
        assert_eq!(eval("exp(0) x 5"), 5.0);
        assert_eq!(evaluate("3x"), None);
        assert_eq!(evaluate("x3"), None);
        assert_eq!(calculation("4x4"), Some(16.0));
    }

    #[test]
    fn spaces_dont_matter() {
        for text in ["5x5", "5*5", "5 x 5", "5 * 5", " 5X5 ", "5×5", "5  *  5"] {
            assert_eq!(calculation(text), Some(25.0), "{text}");
        }
        assert_eq!(calculation("2+3*4"), Some(14.0));
        assert_eq!(calculation("(2+3)x4"), Some(20.0));
        assert_eq!(calculation("2^10"), Some(1024.0));
        assert_eq!(calculation("10-4-3"), Some(3.0));
        assert_eq!(calculation("sqrt(16)x2"), Some(8.0));
    }

    #[test]
    fn order_of_operations() {
        // Brackets, then powers (right to left), then signs, then * / % (left to right), then
        // + - (left to right).
        let cases = [
            ("2 + 3 * 4", 14.0),
            ("2 * 3 + 4", 10.0),
            ("10 - 4 - 3", 3.0),
            ("10 - (4 - 3)", 9.0),
            ("100 / 10 / 5", 2.0),
            ("100 / (10 / 5)", 50.0),
            ("2 * 3 ^ 2", 18.0),
            ("(2 * 3) ^ 2", 36.0),
            ("2 ^ 3 ^ 2", 512.0),
            ("(2 ^ 3) ^ 2", 64.0),
            ("-2 ^ 2", -4.0),
            ("(-2) ^ 2", 4.0),
            ("2 ^ -1", 0.5),
            ("2 - -3", 5.0),
            ("-3 * -3", 9.0),
            ("7 + 10 % 4", 9.0),
            ("10 % 4 * 3", 6.0),
            ("8 / 4 * 2", 4.0),
            ("1 + 2 * 3 - 4 / 2 ^ 2", 6.0),
            ("((1 + 2) * (3 + 4)) / 7", 3.0),
            ("2 * sqrt(9 + 7) ^ 2", 32.0),
        ];
        for (text, expected) in cases {
            assert_eq!(eval(text), expected, "{text}");
        }
    }

    #[test]
    fn knows_numbers_functions_and_constants() {
        assert_eq!(eval("1_000 * 1.5e3"), 1_500_000.0);
        assert_eq!(eval("2 * e"), 2.0 * E);
        assert_eq!(evaluate("2e"), None);
        assert_eq!(eval("sqrt(16) + abs(-2)"), 6.0);
        assert!((eval("sin(pi / 2)") - 1.0).abs() < 1e-12);
        assert_eq!(eval("log(1000)"), 3.0);
        assert_eq!(eval("round(2.5)"), 3.0);
    }

    #[test]
    fn rejects_what_isnt_maths() {
        for text in [
            "", "7zip", "1 +", "(1", "sqrt 4", "foo(1)", "1 / 0", "notepad",
        ] {
            assert_eq!(evaluate(text), None, "{text}");
        }
    }

    #[test]
    fn calculations_need_digits_and_an_operation() {
        assert_eq!(calculation("12 * 3"), Some(36.0));
        assert_eq!(calculation("42"), None);
        assert_eq!(calculation("-5"), None);
        assert_eq!(calculation("pi"), None);
        assert_eq!(calculation("code"), None);
    }

    #[test]
    fn formats_results() {
        assert_eq!(format(36.0), "36");
        assert_eq!(format(0.1 + 0.2), "0.3");
        assert_eq!(format(-0.0), "0");
        assert_eq!(format(2.0 / 3.0), "0.6666666667");
        assert_eq!(format(1e20), "1e20");
    }
}
