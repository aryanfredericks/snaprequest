//! Timing waterfall: every phase drawn as a bar on a shared timeline.

use std::io::{self, Write};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use snapreq_core::model::{TraceEntry, TraceEvent};

/// One phase of one hop, positioned on the execution timeline.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Segment {
    pub label: &'static str,
    pub start: Duration,
    pub end: Duration,
}

/// A request/response round trip; redirects produce several.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hop {
    /// Shown above the hop's bars when there is more than one hop.
    pub title: Option<String>,
    pub segments: Vec<Segment>,
}

fn color(label: &str) -> Style {
    let c = match label {
        "DNS" => AnsiColor::Magenta,
        "connect" => AnsiColor::Yellow,
        "TLS" => AnsiColor::Blue,
        "send" => AnsiColor::Cyan,
        "wait" => AnsiColor::Green,
        _ => AnsiColor::BrightGreen,
    };
    Style::new().fg_color(Some(anstyle::Color::Ansi(c)))
}

/// Splits the trace into hops (at `Redirect` and `AuthChallenge` events) and positions each phase.
/// Phase end times come from the events; start = end − duration.
pub(crate) fn hops(trace: &[TraceEntry]) -> Vec<Hop> {
    let mut hops = vec![Hop {
        title: None,
        segments: Vec::new(),
    }];
    let seg = |label, end: Duration, d: Duration| Segment {
        label,
        start: end.saturating_sub(d),
        end,
    };
    for e in trace {
        let hop = hops.last_mut().expect("at least one hop");
        match &e.event {
            TraceEvent::DnsEnd { duration, .. } => hop.segments.push(seg("DNS", e.at, *duration)),
            TraceEvent::ConnectEnd { duration, .. } => {
                hop.segments.push(seg("connect", e.at, *duration));
            }
            TraceEvent::TlsEnd { duration, .. } => hop.segments.push(seg("TLS", e.at, *duration)),
            TraceEvent::RequestSent { duration, .. } => {
                hop.segments.push(seg("send", e.at, *duration));
            }
            TraceEvent::ResponseFirstByte { wait } => hop.segments.push(seg("wait", e.at, *wait)),
            TraceEvent::ResponseComplete { download, .. } => {
                hop.segments.push(seg("download", e.at, *download));
            }
            TraceEvent::Redirect { status, to, .. } => {
                hop.title = Some(format!("{status} → {to}"));
                hops.push(Hop {
                    title: None,
                    segments: Vec::new(),
                });
            }
            TraceEvent::AuthChallenge { status, scheme } => {
                hop.title = Some(format!("{status} {scheme} challenge"));
                hops.push(Hop {
                    title: None,
                    segments: Vec::new(),
                });
            }
            _ => {}
        }
    }
    if hops.len() > 1 {
        let last = hops.len();
        for (i, hop) in hops.iter_mut().enumerate() {
            let what = hop.title.take().unwrap_or_else(|| "final response".into());
            hop.title = Some(format!("hop {}/{last}: {what}", i + 1));
        }
    }
    hops.retain(|h| !h.segments.is_empty());
    hops
}

fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.2} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// Draws the waterfall `width` columns wide (at least 40).
pub(crate) fn render(out: &mut dyn Write, trace: &[TraceEntry], width: usize) -> io::Result<()> {
    let hops = hops(trace);
    let Some(total) = trace.last().map(|e| e.at) else {
        return Ok(());
    };
    if hops.is_empty() || total.is_zero() {
        return Ok(());
    }

    const LABEL: usize = 9;
    const VALUE: usize = 10;
    let bar_width = width.max(40) - LABEL - VALUE - 2;
    let col = |d: Duration| -> usize {
        let frac = d.as_secs_f64() / total.as_secs_f64();
        ((frac * bar_width as f64).round() as usize).min(bar_width)
    };

    let dim = Style::new().dimmed();
    writeln!(
        out,
        "{dim}{:LABEL$} {:>VALUE$} 0{:>pad$}{dim:#}",
        "",
        "",
        ms(total),
        pad = bar_width - 1
    )?;
    for hop in &hops {
        if let Some(title) = &hop.title {
            writeln!(out, "{dim}{title}{dim:#}")?;
        }
        for s in &hop.segments {
            let start = col(s.start);
            // Every phase is visible, even when it rounds to zero columns.
            let end = col(s.end).max(start + 1).min(bar_width);
            let start = start.min(end - 1);
            let style = color(s.label);
            writeln!(
                out,
                "{:LABEL$} {:>VALUE$} {}{style}{}{style:#}",
                s.label,
                ms(s.end.saturating_sub(s.start)),
                " ".repeat(start),
                "█".repeat(end - start),
            )?;
        }
    }
    Ok(())
}

/// Which output stream the waterfall goes to, for measuring its terminal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

/// Width of the terminal behind `stream`, else `COLUMNS`, else 80.
pub(crate) fn terminal_width(stream: Stream) -> usize {
    let size = match stream {
        Stream::Stdout => terminal_size::terminal_size_of(std::io::stdout()),
        Stream::Stderr => terminal_size::terminal_size_of(std::io::stderr()),
    };
    size.map(|(w, _)| usize::from(w.0))
        .or_else(|| std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()))
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64, event: TraceEvent) -> TraceEntry {
        TraceEntry {
            at: Duration::from_millis(ms),
            event,
        }
    }

    fn d(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn plain(trace: &[TraceEntry], width: usize) -> String {
        let mut buf = Vec::new();
        {
            let mut out = anstream::StripStream::new(&mut buf);
            render(&mut out, trace, width).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    fn one_hop() -> Vec<TraceEntry> {
        vec![
            at(
                10,
                TraceEvent::DnsEnd {
                    host: "a".into(),
                    addrs: vec![],
                    duration: d(10),
                },
            ),
            at(
                20,
                TraceEvent::ConnectEnd {
                    addr: "1.2.3.4:443".parse().unwrap(),
                    local_addr: "5.6.7.8:1".parse().unwrap(),
                    duration: d(10),
                },
            ),
            at(
                40,
                TraceEvent::RequestSent {
                    body_bytes: 0,
                    duration: d(0),
                },
            ),
            at(90, TraceEvent::ResponseFirstByte { wait: d(50) }),
            at(
                100,
                TraceEvent::ResponseComplete {
                    body_bytes: 1,
                    download: d(10),
                },
            ),
        ]
    }

    #[test]
    fn auth_challenges_split_hops() {
        let mut trace = one_hop();
        trace.push(at(
            100,
            TraceEvent::AuthChallenge {
                status: 401,
                scheme: "digest".into(),
            },
        ));
        trace.push(at(
            120,
            TraceEvent::RequestSent {
                body_bytes: 0,
                duration: d(5),
            },
        ));
        let titles: Vec<String> = hops(&trace).into_iter().filter_map(|h| h.title).collect();
        assert_eq!(
            titles,
            ["hop 1/2: 401 digest challenge", "hop 2/2: final response"]
        );
    }

    #[test]
    fn bars_are_placed_on_one_timeline() {
        // 61 columns: 9 label + 10 value + 2 spaces leave 40 columns of bars.
        let text = plain(&one_hop(), 61);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "{text}");
        assert!(
            lines[0].trim_start().starts_with('0') && lines[0].ends_with("100.0 ms"),
            "{text}"
        );
        let bar = |l: &str| {
            let start = l.find('█').unwrap() - 21;
            let len = l.matches('█').count();
            (start, len)
        };
        // 40 columns of bar for 100 ms: DNS 0–10 ms = 4 columns, wait 40–90 ms = 20.
        assert_eq!(bar(lines[1]), (0, 4));
        assert_eq!(bar(lines[2]), (4, 4));
        assert!(lines[3].starts_with("send"));
        assert_eq!(
            bar(lines[3]).1,
            1,
            "zero-length phases still get one column"
        );
        assert_eq!(bar(lines[4]), (16, 20));
        assert_eq!(bar(lines[5]), (36, 4));
        assert!(
            lines[4].starts_with("wait         50.0 ms "),
            "{:?}",
            lines[4]
        );
    }

    #[test]
    fn redirects_make_titled_hops() {
        let mut trace = one_hop();
        trace.push(at(
            100,
            TraceEvent::Redirect {
                status: 302,
                from: "/a".into(),
                to: "/b".into(),
            },
        ));
        trace.push(at(120, TraceEvent::ResponseFirstByte { wait: d(20) }));
        let hops = hops(&trace);
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].title.as_deref(), Some("hop 1/2: 302 → /b"));
        assert_eq!(hops[1].title.as_deref(), Some("hop 2/2: final response"));
        assert_eq!(
            hops[1].segments,
            [Segment {
                label: "wait",
                start: d(100),
                end: d(120)
            }]
        );
    }

    #[test]
    fn empty_trace_draws_nothing() {
        assert_eq!(plain(&[], 80), "");
    }
}
