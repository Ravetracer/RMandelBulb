// SPDX-License-Identifier: LGPL-2.1-or-later
//! Small math expressions for animation drivers ("8 + 2 * sin(t)").
//!
//! Numbers, `+ - * / %`, `^` (power), parentheses, comparisons (`< <= > >= == !=`, 1 or 0),
//! variables (given by the caller, plus `pi` and `e`), functions (see [`call`]) and `@path`
//! for another value of the scene (`@camera.fov`, `@fractal.slots.0.params.power`).

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr(Node);

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Num(f64),
    Var(String),
    Path(String),
    Neg(Box<Node>),
    Bin(char, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
}

/// The functions with their argument counts.
pub const FUNCTIONS: &[(&str, usize)] = &[
    ("sin", 1),
    ("cos", 1),
    ("tan", 1),
    ("asin", 1),
    ("acos", 1),
    ("atan", 1),
    ("atan2", 2),
    ("sqrt", 1),
    ("abs", 1),
    ("exp", 1),
    ("ln", 1),
    ("log10", 1),
    ("pow", 2),
    ("min", 2),
    ("max", 2),
    ("clamp", 3),
    ("mix", 3),
    ("step", 2),
    ("smoothstep", 3),
    ("floor", 1),
    ("ceil", 1),
    ("round", 1),
    ("fract", 1),
    ("sign", 1),
    ("noise", 1),
    ("if", 3),
    ("deg", 1),
    ("rad", 1),
];

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at position {}", self.i + 1))
    }

    /// comparison := sum (op sum)?
    fn comparison(&mut self) -> Result<Node, String> {
        let a = self.sum()?;
        for (t, c) in [("<=", 'l'), (">=", 'g'), ("==", '='), ("!=", '!'), ("<", '<'), (">", '>')] {
            if self.eat(t) {
                let b = self.sum()?;
                return Ok(Node::Bin(c, Box::new(a), Box::new(b)));
            }
        }
        Ok(a)
    }

    fn sum(&mut self) -> Result<Node, String> {
        let mut a = self.product()?;
        loop {
            match self.peek() {
                Some(c @ (b'+' | b'-')) => {
                    self.i += 1;
                    let b = self.product()?;
                    a = Node::Bin(c as char, Box::new(a), Box::new(b));
                }
                _ => return Ok(a),
            }
        }
    }

    fn product(&mut self) -> Result<Node, String> {
        let mut a = self.unary()?;
        loop {
            match self.peek() {
                Some(c @ (b'*' | b'/' | b'%')) => {
                    self.i += 1;
                    let b = self.unary()?;
                    a = Node::Bin(c as char, Box::new(a), Box::new(b));
                }
                _ => return Ok(a),
            }
        }
    }

    fn unary(&mut self) -> Result<Node, String> {
        if self.peek() == Some(b'-') {
            self.i += 1;
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        if self.peek() == Some(b'+') {
            self.i += 1;
            return self.unary();
        }
        self.power()
    }

    /// Right associative: 2^3^2 = 2^9.
    fn power(&mut self) -> Result<Node, String> {
        let base = self.atom()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            let e = self.unary()?;
            return Ok(Node::Bin('^', Box::new(base), Box::new(e)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, String> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let e = self.comparison()?;
                if !self.eat(")") {
                    return self.err("')' expected");
                }
                Ok(e)
            }
            Some(b'@') => {
                self.i += 1;
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'.')) {
                    self.i += 1;
                }
                let p = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("").trim_end_matches('.');
                if p.is_empty() {
                    return self.err("a scene value is expected after @");
                }
                Ok(Node::Path(p.to_string()))
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                    self.i += 1;
                }
                // Exponent: 1e-3.
                if self.i < self.s.len() && matches!(self.s[self.i], b'e' | b'E') {
                    let save = self.i;
                    self.i += 1;
                    if self.i < self.s.len() && matches!(self.s[self.i], b'+' | b'-') {
                        self.i += 1;
                    }
                    if self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                            self.i += 1;
                        }
                    } else {
                        self.i = save;
                    }
                }
                let text = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                text.parse().map(Node::Num).or_else(|_| self.err("bad number"))
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                    self.i += 1;
                }
                let name = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("").to_string();
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.comparison()?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return self.err("',' or ')' expected");
                            }
                        }
                    }
                    match FUNCTIONS.iter().find(|(f, _)| *f == name) {
                        None => Err(format!("unknown function '{name}'")),
                        Some((_, n)) if *n != args.len() => Err(format!("{name} takes {n} value{}", if *n == 1 { "" } else { "s" })),
                        _ => Ok(Node::Call(name, args)),
                    }
                } else {
                    Ok(Node::Var(name))
                }
            }
            Some(c) => self.err(&format!("unexpected '{}'", c as char)),
            None => self.err("value expected"),
        }
    }
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, String> {
        let mut p = Parser { s: src.as_bytes(), i: 0 };
        let e = p.comparison()?;
        if p.peek().is_some() {
            return p.err("unexpected text");
        }
        Ok(Expr(e))
    }

    /// Evaluates with `var` for variables and `path` for `@` scene values (None: unknown).
    pub fn eval(&self, var: &dyn Fn(&str) -> Option<f64>, path: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
        eval(&self.0, var, path)
    }

    /// The scene values (`@paths`) the expression reads.
    pub fn paths(&self) -> Vec<String> {
        fn walk(n: &Node, out: &mut Vec<String>) {
            match n {
                Node::Path(p) => out.push(p.clone()),
                Node::Neg(a) => walk(a, out),
                Node::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Node::Call(_, args) => args.iter().for_each(|a| walk(a, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &mut out);
        out
    }
}

fn eval(n: &Node, var: &dyn Fn(&str) -> Option<f64>, path: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
    Ok(match n {
        Node::Num(v) => *v,
        Node::Var(name) => match name.as_str() {
            "pi" => std::f64::consts::PI,
            "e" => std::f64::consts::E,
            _ => var(name).ok_or_else(|| format!("unknown name '{name}'"))?,
        },
        Node::Path(p) => path(p).ok_or_else(|| format!("no scene value @{p}"))?,
        Node::Neg(a) => -eval(a, var, path)?,
        Node::Bin(op, a, b) => {
            let (x, y) = (eval(a, var, path)?, eval(b, var, path)?);
            let truth = |c: bool| if c { 1.0 } else { 0.0 };
            match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => x / y,
                '%' => x.rem_euclid(y),
                '^' => x.powf(y),
                '<' => truth(x < y),
                '>' => truth(x > y),
                'l' => truth(x <= y),
                'g' => truth(x >= y),
                '=' => truth(x == y),
                _ => truth(x != y),
            }
        }
        Node::Call(name, args) => {
            let a: Vec<f64> = args.iter().map(|a| eval(a, var, path)).collect::<Result<_, _>>()?;
            call(name, &a)
        }
    })
}

/// Smooth 1D value noise in -1..1 (organic wobble): random values at whole numbers, joined by a
/// smooth curve.
pub fn noise(x: f64) -> f64 {
    let hash = |i: i64| {
        let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 31;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 29;
        (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let i = x.floor();
    let f = x - i;
    let s = f * f * (3.0 - 2.0 * f);
    let (a, b) = (hash(i as i64), hash(i as i64 + 1));
    a + (b - a) * s
}

fn call(name: &str, a: &[f64]) -> f64 {
    match name {
        "sin" => a[0].sin(),
        "cos" => a[0].cos(),
        "tan" => a[0].tan(),
        "asin" => a[0].clamp(-1.0, 1.0).asin(),
        "acos" => a[0].clamp(-1.0, 1.0).acos(),
        "atan" => a[0].atan(),
        "atan2" => a[0].atan2(a[1]),
        "sqrt" => a[0].max(0.0).sqrt(),
        "abs" => a[0].abs(),
        "exp" => a[0].exp(),
        "ln" => a[0].ln(),
        "log10" => a[0].log10(),
        "pow" => a[0].powf(a[1]),
        "min" => a[0].min(a[1]),
        "max" => a[0].max(a[1]),
        "clamp" => a[0].clamp(a[1].min(a[2]), a[2].max(a[1])),
        "mix" => a[0] + (a[1] - a[0]) * a[2],
        "step" => {
            if a[1] < a[0] {
                0.0
            } else {
                1.0
            }
        }
        "smoothstep" => {
            let t = ((a[2] - a[0]) / (a[1] - a[0])).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
        "floor" => a[0].floor(),
        "ceil" => a[0].ceil(),
        "round" => a[0].round(),
        "fract" => a[0] - a[0].floor(),
        "sign" => a[0].signum(),
        "noise" => noise(a[0]),
        "if" => {
            if a[0] > 0.0 {
                a[1]
            } else {
                a[2]
            }
        }
        "deg" => a[0].to_degrees(),
        "rad" => a[0].to_radians(),
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        let vars = |n: &str| match n {
            "t" => Some(2.0),
            "value" => Some(8.0),
            _ => None,
        };
        let paths = |p: &str| (p == "camera.fov").then_some(40.0);
        Expr::parse(s).unwrap().eval(&vars, &paths).unwrap()
    }

    #[test]
    fn arithmetic_functions_variables_and_paths() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(ev("-2 ^ 2"), -4.0);
        assert_eq!(ev("value + t * 0.5"), 9.0);
        assert_eq!(ev("@camera.fov / 2"), 20.0);
        assert_eq!(ev("clamp(5, 0, 1) + if(t > 1, 10, 20)"), 11.0);
        assert_eq!(ev("1e-3 * 1000"), 1.0);
        assert!((ev("sin(pi / 2)") - 1.0).abs() < 1e-12);
        assert_eq!(ev("7 % 3"), 1.0);
        assert!(ev("noise(t)").abs() <= 1.0);
        assert_eq!(noise(3.0), noise(3.0));
        assert_eq!(Expr::parse("@a.b + @c").unwrap().paths(), vec!["a.b".to_string(), "c".to_string()]);
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert!(Expr::parse("1 +").unwrap_err().contains("value expected"));
        assert!(Expr::parse("foo(1)").unwrap_err().contains("unknown function"));
        assert!(Expr::parse("min(1)").unwrap_err().contains("takes 2"));
        assert!(Expr::parse("(1").unwrap_err().contains("')'"));
        let e = Expr::parse("x + 1").unwrap();
        assert!(e.eval(&|_| None, &|_| None).unwrap_err().contains("unknown name 'x'"));
    }
}
