//! A tolerant RFC 4180 reader, enough for GTFS text files.

pub struct Reader<R: std::io::BufRead> {
    inner: R,
    line: String,
    pub header: Vec<String>,
    fields: Vec<String>,
}

/// Splits one record. Iterating over characters rather than bytes matters:
/// half the accented stop names in the Rio feed sit inside quoted fields, and
/// copying those byte by byte turns "Afrânio" into "AfrÃ¢nio".
fn split_line(line: &str, out: &mut Vec<String>) {
    out.clear();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
}

impl<R: std::io::BufRead> Reader<R> {
    pub fn new(mut inner: R) -> anyhow::Result<Self> {
        let mut line = String::new();
        inner.read_line(&mut line)?;
        let trimmed = line.trim_start_matches('\u{feff}').trim_end_matches(['\r', '\n']);
        let mut header = Vec::new();
        split_line(trimmed, &mut header);
        Ok(Self {
            inner,
            line: String::new(),
            header,
            fields: Vec::new(),
        })
    }

    pub fn column(&self, name: &str) -> Option<usize> {
        self.header.iter().position(|h| h == name)
    }

    /// Reads the next record, returning the raw field slice.
    pub fn next_record(&mut self) -> anyhow::Result<Option<&[String]>> {
        loop {
            self.line.clear();
            let n = self.inner.read_line(&mut self.line)?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = self.line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                continue;
            }
            // A quoted field may contain a newline; keep reading while quotes are unbalanced.
            let mut owned = trimmed.to_string();
            while owned.matches('"').count() % 2 == 1 {
                self.line.clear();
                if self.inner.read_line(&mut self.line)? == 0 {
                    break;
                }
                owned.push('\n');
                owned.push_str(self.line.trim_end_matches(['\r', '\n']));
            }
            split_line(&owned, &mut self.fields);
            return Ok(Some(&self.fields));
        }
    }
}

pub fn get<'a>(fields: &'a [String], idx: Option<usize>) -> &'a str {
    idx.and_then(|i| fields.get(i)).map(|s| s.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_and_accented_fields() {
        let data = "\u{feff}a,b,c\n1,\"x,y\",z\n2,\"he said \"\"hi\"\"\",ç\n\n3,,ok\n\"BRS 1,2,3: Afrânio\",Praça,Góis\n";
        let mut r = Reader::new(std::io::Cursor::new(data)).unwrap();
        assert_eq!(r.header, vec!["a", "b", "c"]);
        assert_eq!(r.column("b"), Some(1));
        let rec = r.next_record().unwrap().unwrap().to_vec();
        assert_eq!(rec, vec!["1", "x,y", "z"]);
        let rec = r.next_record().unwrap().unwrap().to_vec();
        assert_eq!(rec, vec!["2", "he said \"hi\"", "ç"]);
        let rec = r.next_record().unwrap().unwrap().to_vec();
        assert_eq!(rec, vec!["3", "", "ok"]);
        // Accents inside a quoted field are the case that broke 122 Rio stop names.
        let rec = r.next_record().unwrap().unwrap().to_vec();
        assert_eq!(rec, vec!["BRS 1,2,3: Afrânio", "Praça", "Góis"]);
        assert!(r.next_record().unwrap().is_none());
    }
}
