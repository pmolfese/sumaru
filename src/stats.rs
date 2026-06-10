#[derive(Debug, Clone, PartialEq)]
pub struct AfniStatSpec {
    pub name: String,
    pub params: Vec<f64>,
}

impl AfniStatSpec {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            return None;
        }

        let (name, params) = match trimmed.split_once('(') {
            Some((name, rest)) => {
                let params = rest.trim_end_matches(')').trim();
                let params = if params.is_empty() {
                    Vec::new()
                } else {
                    params
                        .split(',')
                        .map(str::trim)
                        .filter(|piece| !piece.is_empty())
                        .map(str::parse::<f64>)
                        .collect::<Result<Vec<_>, _>>()
                        .ok()?
                };
                (name.trim(), params)
            }
            None => (trimmed, Vec::new()),
        };

        if name.is_empty() {
            return None;
        }

        Some(Self {
            name: name.to_string(),
            params,
        })
    }

    pub fn two_sided_p_value(&self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }

        match compact_lower(&self.name).as_str() {
            "ttest" if !self.params.is_empty() => t_two_tailed_p_value(value, self.params[0]),
            "ftest" if self.params.len() >= 2 => {
                f_upper_tail_p_value(value, self.params[0], self.params[1])
            }
            "correlation" | "correl" if !self.params.is_empty() => {
                correlation_two_tailed_p_value(value, self.params[0])
            }
            "fisherz" | "zscore" => normal_two_tailed_p_value(value),
            "chisq" | "chi2" if !self.params.is_empty() => {
                chi_square_upper_tail_p_value(value, self.params[0])
            }
            _ => None,
        }
    }

    pub fn statistic_for_p_value(&self, p_value: f64) -> Option<f64> {
        if !(0.0..=1.0).contains(&p_value) || !p_value.is_finite() {
            return None;
        }
        if p_value == 1.0 {
            return Some(0.0);
        }
        if p_value == 0.0 {
            return None;
        }

        let mut high = 1.0;
        for _ in 0..128 {
            if self.two_sided_p_value(high)? <= p_value {
                break;
            }
            high *= 2.0;
            if !high.is_finite() || high > 1.0e12 {
                return None;
            }
        }
        if self.two_sided_p_value(high)? > p_value {
            return None;
        }

        let mut low = 0.0;
        for _ in 0..96 {
            let midpoint = (low + high) * 0.5;
            if self.two_sided_p_value(midpoint)? <= p_value {
                high = midpoint;
            } else {
                low = midpoint;
            }
        }

        Some(high)
    }
}

fn t_two_tailed_p_value(t: f64, df: f64) -> Option<f64> {
    if df <= 0.0 || !df.is_finite() {
        return None;
    }

    let x = df / (df + t * t);
    regularized_beta(x, df * 0.5, 0.5).map(|p| p.clamp(0.0, 1.0))
}

fn f_upper_tail_p_value(f: f64, df_num: f64, df_den: f64) -> Option<f64> {
    if f < 0.0 || df_num <= 0.0 || df_den <= 0.0 {
        return None;
    }

    let x = df_den / (df_den + df_num * f);
    regularized_beta(x, df_den * 0.5, df_num * 0.5).map(|p| p.clamp(0.0, 1.0))
}

fn correlation_two_tailed_p_value(r: f64, n_or_df: f64) -> Option<f64> {
    let df = (n_or_df - 2.0).max(1.0);
    let clipped = r.clamp(-0.999_999, 0.999_999);
    let t = clipped * (df / (1.0 - clipped * clipped).max(1.0e-12)).sqrt();
    t_two_tailed_p_value(t, df)
}

fn normal_two_tailed_p_value(z: f64) -> Option<f64> {
    Some(erfc(z.abs() / std::f64::consts::SQRT_2).clamp(0.0, 1.0))
}

fn chi_square_upper_tail_p_value(value: f64, df: f64) -> Option<f64> {
    if value < 0.0 || df <= 0.0 {
        return None;
    }

    regularized_gamma_q(df * 0.5, value * 0.5).map(|p| p.clamp(0.0, 1.0))
}

fn regularized_beta(x: f64, a: f64, b: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&x) || a <= 0.0 || b <= 0.0 {
        return None;
    }
    if x == 0.0 {
        return Some(0.0);
    }
    if x == 1.0 {
        return Some(1.0);
    }

    let log_bt = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    let bt = log_bt.exp();

    if x < (a + 1.0) / (a + b + 2.0) {
        Some(bt * beta_continued_fraction(a, b, x)? / a)
    } else {
        Some(1.0 - bt * beta_continued_fraction(b, a, 1.0 - x)? / b)
    }
}

fn beta_continued_fraction(a: f64, b: f64, x: f64) -> Option<f64> {
    const MAX_ITERATIONS: usize = 200;
    const EPSILON: f64 = 3.0e-14;
    const FP_MIN: f64 = 1.0e-300;

    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FP_MIN {
        d = FP_MIN;
    }
    d = 1.0 / d;
    let mut h = d;

    for m in 1..=MAX_ITERATIONS {
        let m2 = 2 * m;
        let m_f = m as f64;
        let m2_f = m2 as f64;

        let mut aa = m_f * (b - m_f) * x / ((qam + m2_f) * (a + m2_f));
        d = 1.0 + aa * d;
        if d.abs() < FP_MIN {
            d = FP_MIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FP_MIN {
            c = FP_MIN;
        }
        d = 1.0 / d;
        h *= d * c;

        aa = -(a + m_f) * (qab + m_f) * x / ((a + m2_f) * (qap + m2_f));
        d = 1.0 + aa * d;
        if d.abs() < FP_MIN {
            d = FP_MIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FP_MIN {
            c = FP_MIN;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;

        if (delta - 1.0).abs() < EPSILON {
            return Some(h);
        }
    }

    Some(h)
}

fn regularized_gamma_q(a: f64, x: f64) -> Option<f64> {
    if a <= 0.0 || x < 0.0 {
        return None;
    }
    if x == 0.0 {
        return Some(1.0);
    }
    if x < a + 1.0 {
        regularized_gamma_p_series(a, x).map(|p| 1.0 - p)
    } else {
        regularized_gamma_q_continued_fraction(a, x)
    }
}

fn regularized_gamma_p_series(a: f64, x: f64) -> Option<f64> {
    const MAX_ITERATIONS: usize = 200;
    const EPSILON: f64 = 3.0e-14;

    let gln = ln_gamma(a);
    let mut ap = a;
    let mut sum = 1.0 / a;
    let mut delta = sum;

    for _ in 0..MAX_ITERATIONS {
        ap += 1.0;
        delta *= x / ap;
        sum += delta;
        if delta.abs() < sum.abs() * EPSILON {
            return Some(sum * (-x + a * x.ln() - gln).exp());
        }
    }

    Some(sum * (-x + a * x.ln() - gln).exp())
}

fn regularized_gamma_q_continued_fraction(a: f64, x: f64) -> Option<f64> {
    const MAX_ITERATIONS: usize = 200;
    const EPSILON: f64 = 3.0e-14;
    const FP_MIN: f64 = 1.0e-300;

    let gln = ln_gamma(a);
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / FP_MIN;
    let mut d = 1.0 / b.max(FP_MIN);
    let mut h = d;

    for i in 1..=MAX_ITERATIONS {
        let i_f = i as f64;
        let an = -i_f * (i_f - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < FP_MIN {
            d = FP_MIN;
        }
        c = b + an / c;
        if c.abs() < FP_MIN {
            c = FP_MIN;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPSILON {
            break;
        }
    }

    Some(h * (-x + a * x.ln() - gln).exp())
}

fn ln_gamma(z: f64) -> f64 {
    const COEFFICIENTS: [f64; 8] = [
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    if z < 0.5 {
        return std::f64::consts::PI.ln()
            - (std::f64::consts::PI * z).sin().ln()
            - ln_gamma(1.0 - z);
    }

    let z = z - 1.0;
    let mut x = 0.999_999_999_999_809_9;
    for (index, coefficient) in COEFFICIENTS.iter().enumerate() {
        x += coefficient / (z + index as f64 + 1.0);
    }
    let t = z + 7.5;

    (2.0 * std::f64::consts::PI).sqrt().ln() + (z + 0.5) * t.ln() - t + x.ln()
}

fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let value = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();

    if x >= 0.0 { value } else { 2.0 - value }
}

fn compact_lower(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::AfniStatSpec;

    #[test]
    fn parses_afni_stat_labels() {
        let spec = AfniStatSpec::parse("Ttest(48)").unwrap();

        assert_eq!(spec.name, "Ttest");
        assert_eq!(spec.params, vec![48.0]);
        assert!(AfniStatSpec::parse("none").is_none());
    }

    #[test]
    fn ttest_p_values_match_reference_points() {
        let spec = AfniStatSpec::parse("Ttest(48)").unwrap();

        assert_close(spec.two_sided_p_value(0.0).unwrap(), 1.0, 0.000_001);
        assert_close(spec.two_sided_p_value(2.010_635).unwrap(), 0.05, 0.000_1);
        assert!(spec.two_sided_p_value(8.997_519).unwrap() < 1.0e-10);
        assert_close(
            spec.statistic_for_p_value(0.05).unwrap(),
            2.010_635,
            0.000_1,
        );
    }

    #[test]
    fn f_z_and_chi_square_p_values_match_reference_points() {
        let f = AfniStatSpec::parse("Ftest(1,48)").unwrap();
        assert_close(f.two_sided_p_value(4.042_652).unwrap(), 0.05, 0.000_1);
        assert_close(f.statistic_for_p_value(0.05).unwrap(), 4.042_652, 0.000_1);

        let z = AfniStatSpec::parse("Zscore()").unwrap();
        assert_close(z.two_sided_p_value(1.959_964).unwrap(), 0.05, 0.000_1);
        assert_close(z.statistic_for_p_value(0.05).unwrap(), 1.959_964, 0.000_1);

        let chi = AfniStatSpec::parse("ChiSq(1)").unwrap();
        assert_close(chi.two_sided_p_value(3.841_459).unwrap(), 0.05, 0.000_1);
        assert_close(chi.statistic_for_p_value(0.05).unwrap(), 3.841_459, 0.000_1);
    }

    #[test]
    fn invalid_or_impossible_p_value_thresholds_are_rejected() {
        let spec = AfniStatSpec::parse("Ttest(48)").unwrap();

        assert_eq!(spec.statistic_for_p_value(1.0), Some(0.0));
        assert!(spec.statistic_for_p_value(0.0).is_none());
        assert!(spec.statistic_for_p_value(-0.1).is_none());
        assert!(spec.statistic_for_p_value(1.1).is_none());
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {actual} to be within {tolerance} of {expected}"
        );
    }
}
