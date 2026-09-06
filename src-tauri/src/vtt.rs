use crate::BeaError;

#[derive(Debug, Clone, PartialEq)]
pub struct VttCue {
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub speaker: Option<String>,
    pub text: String,
}

fn timestamp_seconds(raw: &str) -> Result<u64, BeaError> {
    // Teams uses HH:MM:SS.mmm; the WebVTT spec also allows MM:SS.mmm.
    let parts: Vec<&str> = raw.trim().split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => ("0", *m, *s),
        _ => {
            return Err(BeaError::UnsupportedMedia(format!(
                "bad VTT timestamp: {raw}"
            )))
        }
    };
    let hours: u64 = hours
        .trim()
        .parse()
        .map_err(|_| BeaError::UnsupportedMedia(format!("bad VTT timestamp: {raw}")))?;
    let minutes: u64 = minutes
        .parse()
        .map_err(|_| BeaError::UnsupportedMedia(format!("bad VTT timestamp: {raw}")))?;
    let seconds_f: f64 = seconds
        .parse()
        .map_err(|_| BeaError::UnsupportedMedia(format!("bad VTT timestamp: {raw}")))?;
    Ok(hours * 3600 + minutes * 60 + seconds_f.round() as u64)
}

fn strip_speaker_tag(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("<v ") {
        if let Some((name, remainder)) = rest.split_once('>') {
            return (Some(name.trim().to_string()), remainder.trim().to_string());
        }
    }
    (None, trimmed.to_string())
}

pub fn parse_vtt(input: &str) -> Result<Vec<VttCue>, BeaError> {
    if !input.trim_start().starts_with("WEBVTT") {
        return Err(BeaError::UnsupportedMedia(
            "file is not a WebVTT document".into(),
        ));
    }
    let mut cues = Vec::new();
    let mut current: Option<(u64, u64, Option<String>, Vec<String>)> = None;
    for line in input.lines() {
        let line = line.trim_end_matches('\r').trim().to_string();
        if line.is_empty() {
            if let Some((start, end, speaker, text_lines)) = current.take() {
                let text = text_lines.join(" ").trim().to_string();
                if !text.is_empty() {
                    cues.push(VttCue {
                        start_seconds: start,
                        end_seconds: end,
                        speaker,
                        text,
                    });
                }
            }
            continue;
        }
        if let Some((start_raw, end_raw)) = line.split_once("-->") {
            current = Some((
                timestamp_seconds(start_raw)?,
                timestamp_seconds(end_raw)?,
                None,
                Vec::new(),
            ));
            continue;
        }
        if line.starts_with("WEBVTT")
            || line.starts_with("NOTE")
            || line.starts_with("STYLE")
            || line.starts_with("Kind:")
            || line.starts_with("Language:")
        {
            continue;
        }
        if let Some((_, _, _, text_lines)) = current.as_mut() {
            // A bare cue-identifier line (no "-->", no prior text) is skipped.
            if text_lines.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if text_lines.is_empty() {
                let (speaker, remainder) = strip_speaker_tag(&line);
                if let Some(speaker) = speaker {
                    let (_, _, slot, text_lines) = current.as_mut().unwrap();
                    *slot = Some(speaker);
                    if !remainder.is_empty() {
                        text_lines.push(remainder);
                    }
                    continue;
                }
            }
            text_lines.push(line);
        }
    }
    if let Some((start, end, speaker, text_lines)) = current.take() {
        let text = text_lines.join(" ").trim().to_string();
        if !text.is_empty() {
            cues.push(VttCue {
                start_seconds: start,
                end_seconds: end,
                speaker,
                text,
            });
        }
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_teams_vtt_with_speakers_and_offsets() {
        let vtt = "WEBVTT\r\n\r\n00:00:05.120 --> 00:00:08.400\r\n<v Maria Santos>Good morning everyone.\r\n\r\n00:00:08.400 --> 00:00:12.000\r\n<v John Cruz>I think we should ship it.\r\n\r\n00:01:00.000 --> 00:01:02.500\r\n<v Maria Santos>Let's vote.\r\n";
        let cues = parse_vtt(vtt).unwrap();
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].start_seconds, 5);
        assert_eq!(cues[0].end_seconds, 8);
        assert_eq!(cues[0].speaker.as_deref(), Some("Maria Santos"));
        assert_eq!(cues[0].text, "Good morning everyone.");
        assert_eq!(cues[2].start_seconds, 60);
        assert!(cues[2].speaker.is_some());
    }

    #[test]
    fn parses_cues_without_speaker_tags() {
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nPlain narration line.\n";
        let cues = parse_vtt(vtt).unwrap();
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].speaker, None);
        assert_eq!(cues[0].text, "Plain narration line.");
    }

    #[test]
    fn rejects_non_vtt_input() {
        assert!(parse_vtt("not a vtt").is_err());
    }
}
