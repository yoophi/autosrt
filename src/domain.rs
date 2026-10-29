use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Input {
    Youtube(String),
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub fn render_srt(segments: &[Segment]) -> Result<String, String> {
    let mut result = String::new();
    let mut previous_end = 0;
    let mut index = 0;
    for s in segments {
        let text = s
            .text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            continue;
        }
        if s.start_ms >= s.end_ms || s.start_ms < previous_end {
            return Err(format!(
                "Invalid or overlapping segment: {}..{}",
                s.start_ms, s.end_ms
            ));
        }
        index += 1;
        result.push_str(&format!(
            "{index}\n{} --> {}\n{text}\n\n",
            timestamp(s.start_ms),
            timestamp(s.end_ms)
        ));
        previous_end = s.end_ms;
    }
    if index == 0 {
        return Err("NoSpeech: no non-empty speech segments".into());
    }
    Ok(result)
}

fn timestamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn korean_and_hour_boundary() {
        let s = render_srt(&[Segment {
            start_ms: 3_599_999,
            end_ms: 3_601_001,
            text: " 안녕하세요\n\n세계 ".into(),
        }])
        .unwrap();
        assert_eq!(s, "1\n00:59:59,999 --> 01:00:01,001\n안녕하세요\n세계\n\n");
    }
    #[test]
    fn rejects_empty_reversed_and_overlapping() {
        assert!(render_srt(&[]).is_err());
        assert!(
            render_srt(&[Segment {
                start_ms: 2,
                end_ms: 1,
                text: "a".into()
            }])
            .is_err()
        );
        assert!(
            render_srt(&[
                Segment {
                    start_ms: 0,
                    end_ms: 2,
                    text: "a".into()
                },
                Segment {
                    start_ms: 1,
                    end_ms: 3,
                    text: "b".into()
                }
            ])
            .is_err()
        );
    }
}
