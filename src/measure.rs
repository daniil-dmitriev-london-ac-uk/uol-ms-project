use std::io::Write;
use std::path::Path;



pub fn prc_ns(samples: &mut [u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    let index = ((samples.len() - 1) as f64 * percentile).round() as usize;

    samples[index]
}



#[derive(Debug, Default, Clone, Copy)]
pub struct OperationStats {
    pub iterations: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub total_ns: u64,
}

impl OperationStats {
    pub fn from_samples(samples: &mut [u64]) -> OperationStats {
        let total: u64 = samples.iter().sum();

        OperationStats {
            iterations: samples.len() as u64,
            p50_ns: prc_ns(samples, 0.50),
            p95_ns: prc_ns(samples, 0.95),
            p99_ns: prc_ns(samples, 0.99),
            total_ns: total,
        }
    }

    
    pub fn ops_per_sec(&self) -> f64 {
        if self.total_ns == 0 { return 0.0; }
        self.iterations as f64 / (self.total_ns as f64 / 1e9)
    }


}


pub fn median_u64(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}
pub fn median_f64(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}


pub struct Csv { file: std::fs::File }

impl Csv {
    pub fn open(path: &Path, header: &str) -> std::io::Result<Csv> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let new_file = !path.exists();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        if new_file {
            writeln!(file, "{header}")?;
        }

        Ok(Csv { file })

    }

    
    pub fn row(&mut self, fields: &[String]) -> std::io::Result<()> {
        writeln!(self.file, "{}", fields.join(","))
    }




}
