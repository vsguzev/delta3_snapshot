use clap::Parser;
use std::str::FromStr;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(long)]
    pub mainnet: bool,

    pub symbol: String,

    #[arg(short, long)]
    pub risk: f64,

    #[arg(short, long)]
    pub funds: f64,

    #[arg(short, long, value_parser = parse_duration)]
    pub time: Duration,
}

#[derive(Debug, Clone)]
pub struct Duration {
    pub secs: u64,
}

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} seconds", self.secs)
    }
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    if let Some(stripped) = s.strip_suffix('s') {
        let num = u64::from_str(stripped).map_err(|e| e.to_string())?;
        Ok(Duration { secs: num })
    } else if let Some(stripped) = s.strip_suffix('m') {
        let num = u64::from_str(stripped).map_err(|e| e.to_string())?;
        Ok(Duration { secs: num * 60 })
    } else if let Some(stripped) = s.strip_suffix('h') {
        let num = u64::from_str(stripped).map_err(|e| e.to_string())?;
        Ok(Duration { secs: num * 3600 })
    } else {
        Err("Invalid duration format. Use formats like 1s, 3m, or 1h.".into())
    }
}
