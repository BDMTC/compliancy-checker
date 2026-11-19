//! ISO section/clause presence checklists (9001 / 14001 / 45001).
//!
//! v1 is intentionally weak vs auditor judgement: we look for clause numbers
//! and/or section title keywords in extracted `.docx` text. No LLM calls.

use std::fmt::Write as _;
use std::path::Path;

use crate::extract::{extract_docx_text, DocText};
use crate::{DocxError, Result};

/// Supported ISO management-system standards (UI values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsoStandard {
    Iso9001,
    Iso14001,
    Iso45001,
}

impl IsoStandard {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Iso9001 => "9001",
            Self::Iso14001 => "14001",
            Self::Iso45001 => "45001",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Iso9001 => "ISO 9001",
            Self::Iso14001 => "ISO 14001",
            Self::Iso45001 => "ISO 45001",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().to_lowercase();
        let s = s.strip_prefix("iso").unwrap_or(&s).trim();
        match s {
            "9001" => Some(Self::Iso9001),
            "14001" => Some(Self::Iso14001),
            "45001" => Some(Self::Iso45001),
            _ => None,
        }
    }
}

/// One bundled checklist row (clause / control).
#[derive(Debug, Clone, Copy)]
pub struct ChecklistClause {
    pub standard: IsoStandard,
    pub clause: &'static str,
    pub title: &'static str,
    /// Case-insensitive substrings; any hit ⇒ present.
    pub patterns: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Missing,
}

impl Presence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub document: String,
    pub standard: IsoStandard,
    pub clause: String,
    pub title: String,
    pub status: Presence,
    pub matched_pattern: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChecklistReport {
    pub findings: Vec<Finding>,
    pub standards: Vec<IsoStandard>,
}

impl ChecklistReport {
    pub fn present_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.status == Presence::Present)
            .count()
    }

    pub fn missing_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.status == Presence::Missing)
            .count()
    }

    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "document,standard,clause,title,status,matched_pattern\n",
        );
        for f in &self.findings {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{}",
                csv_escape(&f.document),
                f.standard.as_str(),
                csv_escape(&f.clause),
                csv_escape(&f.title),
                f.status.as_str(),
                csv_escape(f.matched_pattern.as_deref().unwrap_or("")),
            );
        }
        out
    }

    pub fn to_text_report(&self) -> String {
        let mut out = String::new();
        out.push_str("Compliancy Checker — ISO section checklist\n");
        out.push_str(
            "Presence only (keyword / heading match). Not an accreditation decision.\n",
        );
        let standards = self
            .standards
            .iter()
            .map(|s| s.label())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "Standards: {standards}");
        let _ = writeln!(
            out,
            "Summary: {} present, {} missing\n",
            self.present_count(),
            self.missing_count()
        );

        let mut current_doc: Option<&str> = None;
        let mut current_std: Option<IsoStandard> = None;
        for f in &self.findings {
            if current_doc != Some(f.document.as_str()) {
                current_doc = Some(&f.document);
                current_std = None;
                let _ = writeln!(out, "Document: {}", f.document);
            }
            if current_std != Some(f.standard) {
                current_std = Some(f.standard);
                let _ = writeln!(out, "  {}", f.standard.label());
            }
            let mark = match f.status {
                Presence::Present => "PASS",
                Presence::Missing => "MISS",
            };
            let _ = writeln!(out, "    [{mark}] {} {}", f.clause, f.title);
        }
        out
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Bundled clauses for the selected standards.
pub fn clauses_for(standards: &[IsoStandard]) -> Vec<&'static ChecklistClause> {
    CATALOG
        .iter()
        .filter(|c| standards.contains(&c.standard))
        .collect()
}

/// Run checklist against already-extracted document text.
pub fn evaluate_text(
    document_label: &str,
    text: &DocText,
    standards: &[IsoStandard],
) -> ChecklistReport {
    let blob = text.search_blob();
    let mut findings = Vec::new();
    for clause in clauses_for(standards) {
        let mut matched = None;
        for pat in clause.patterns {
            if blob_matches(&blob, pat) {
                matched = Some((*pat).to_string());
                break;
            }
        }
        findings.push(Finding {
            document: document_label.to_string(),
            standard: clause.standard,
            clause: clause.clause.to_string(),
            title: clause.title.to_string(),
            status: if matched.is_some() {
                Presence::Present
            } else {
                Presence::Missing
            },
            matched_pattern: matched,
        });
    }
    ChecklistReport {
        findings,
        standards: standards.to_vec(),
    }
}

/// Clause ids like `6.1` must not match as a prefix of `6.1.2`.
fn is_clause_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn blob_matches(blob: &str, pat: &str) -> bool {
    let p = pat.to_lowercase();
    if !is_clause_id(&p) {
        return blob.contains(&p);
    }
    let mut rest = blob;
    while let Some(idx) = rest.find(&p) {
        let after = &rest[idx + p.len()..];
        let ok = match after.chars().next() {
            None => true,
            Some(c) if c.is_ascii_digit() => false,
            Some('.') => !matches!(after.chars().nth(1), Some(c) if c.is_ascii_digit()),
            _ => true,
        };
        if ok {
            return true;
        }
        rest = &rest[idx + 1..];
    }
    false
}

/// Extract text from `docx_path` and evaluate selected standards.
pub fn evaluate_docx(
    docx_path: &Path,
    standards: &[IsoStandard],
) -> Result<ChecklistReport> {
    if standards.is_empty() {
        return Ok(ChecklistReport {
            findings: Vec::new(),
            standards: Vec::new(),
        });
    }
    let text = extract_docx_text(docx_path)?;
    let label = docx_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.docx");
    Ok(evaluate_text(label, &text, standards))
}

/// Merge multiple per-document reports (same standards list).
pub fn merge_reports(reports: Vec<ChecklistReport>) -> ChecklistReport {
    let mut standards = Vec::new();
    let mut findings = Vec::new();
    for r in reports {
        for s in r.standards {
            if !standards.contains(&s) {
                standards.push(s);
            }
        }
        findings.extend(r.findings);
    }
    ChecklistReport {
        findings,
        standards,
    }
}

/// Write CSV + text report beside processed documents.
pub fn write_exports(report: &ChecklistReport, output_dir: &Path) -> Result<(String, String)> {
    std::fs::create_dir_all(output_dir)?;
    let csv_path = output_dir.join("iso-checklist.csv");
    let txt_path = output_dir.join("iso-checklist-report.txt");
    std::fs::write(&csv_path, report.to_csv())?;
    std::fs::write(&txt_path, report.to_text_report())?;
    Ok((
        csv_path.display().to_string(),
        txt_path.display().to_string(),
    ))
}

/// Parse UI standard strings; unknown values become an error message list.
pub fn parse_standards(raw: &[String]) -> Result<Vec<IsoStandard>> {
    let mut out = Vec::new();
    let mut unknown = Vec::new();
    for s in raw {
        match IsoStandard::parse(s) {
            Some(std) => {
                if !out.contains(&std) {
                    out.push(std);
                }
            }
            None => unknown.push(s.clone()),
        }
    }
    if !unknown.is_empty() {
        return Err(DocxError::Message(format!(
            "unknown ISO standard id(s): {}",
            unknown.join(", ")
        )));
    }
    Ok(out)
}

macro_rules! clause {
    ($std:ident, $id:literal, $title:literal, [$($pat:literal),+ $(,)?]) => {
        ChecklistClause {
            standard: IsoStandard::$std,
            clause: $id,
            title: $title,
            patterns: &[$($pat),+],
        }
    };
}

/// High-level Annex SL + standard-specific section presence checklist.
static CATALOG: &[ChecklistClause] = &[
    // —— ISO 9001 ——
    clause!(Iso9001, "4.1", "Understanding the organization and its context",
        ["4.1", "context of the organization", "organisational context", "organizational context"]),
    clause!(Iso9001, "4.2", "Needs and expectations of interested parties",
        ["4.2", "interested parties", "needs and expectations"]),
    clause!(Iso9001, "4.3", "Determining the scope of the quality management system",
        ["4.3", "scope of the quality management system", "qms scope", "scope of the qms"]),
    clause!(Iso9001, "4.4", "Quality management system and its processes",
        ["4.4", "quality management system and its processes", "qms processes"]),
    clause!(Iso9001, "5.1", "Leadership and commitment",
        ["5.1", "leadership and commitment"]),
    clause!(Iso9001, "5.2", "Policy",
        ["5.2", "quality policy"]),
    clause!(Iso9001, "5.3", "Organizational roles, responsibilities and authorities",
        ["5.3", "roles, responsibilities", "roles and responsibilities"]),
    clause!(Iso9001, "6.1", "Actions to address risks and opportunities",
        ["6.1", "risks and opportunities"]),
    clause!(Iso9001, "6.2", "Quality objectives and planning to achieve them",
        ["6.2", "quality objectives"]),
    clause!(Iso9001, "6.3", "Planning of changes",
        ["6.3", "planning of changes"]),
    clause!(Iso9001, "7.1", "Resources",
        ["7.1", "resources"]),
    clause!(Iso9001, "7.2", "Competence",
        ["7.2", "competence"]),
    clause!(Iso9001, "7.3", "Awareness",
        ["7.3", "awareness"]),
    clause!(Iso9001, "7.4", "Communication",
        ["7.4", "communication"]),
    clause!(Iso9001, "7.5", "Documented information",
        ["7.5", "documented information"]),
    clause!(Iso9001, "8.1", "Operational planning and control",
        ["8.1", "operational planning and control"]),
    clause!(Iso9001, "8.2", "Requirements for products and services",
        ["8.2", "requirements for products and services", "customer requirements"]),
    clause!(Iso9001, "8.3", "Design and development of products and services",
        ["8.3", "design and development"]),
    clause!(Iso9001, "8.4", "Control of externally provided processes, products and services",
        ["8.4", "externally provided", "control of externally provided"]),
    clause!(Iso9001, "8.5", "Production and service provision",
        ["8.5", "production and service provision"]),
    clause!(Iso9001, "8.6", "Release of products and services",
        ["8.6", "release of products and services"]),
    clause!(Iso9001, "8.7", "Control of nonconforming outputs",
        ["8.7", "nonconforming outputs", "non-conforming outputs"]),
    clause!(Iso9001, "9.1", "Monitoring, measurement, analysis and evaluation",
        ["9.1", "monitoring, measurement", "monitoring measurement analysis"]),
    clause!(Iso9001, "9.2", "Internal audit",
        ["9.2", "internal audit"]),
    clause!(Iso9001, "9.3", "Management review",
        ["9.3", "management review"]),
    clause!(Iso9001, "10.2", "Nonconformity and corrective action",
        ["10.2", "nonconformity and corrective action", "corrective action"]),
    clause!(Iso9001, "10.3", "Continual improvement",
        ["10.3", "continual improvement"]),

    // —— ISO 14001 ——
    clause!(Iso14001, "4.1", "Understanding the organization and its context",
        ["4.1", "context of the organization", "organisational context", "organizational context"]),
    clause!(Iso14001, "4.2", "Needs and expectations of interested parties",
        ["4.2", "interested parties"]),
    clause!(Iso14001, "4.3", "Determining the scope of the environmental management system",
        ["4.3", "scope of the environmental management system", "ems scope", "scope of the ems"]),
    clause!(Iso14001, "4.4", "Environmental management system",
        ["4.4", "environmental management system"]),
    clause!(Iso14001, "5.1", "Leadership and commitment",
        ["5.1", "leadership and commitment"]),
    clause!(Iso14001, "5.2", "Environmental policy",
        ["5.2", "environmental policy"]),
    clause!(Iso14001, "5.3", "Organizational roles, responsibilities and authorities",
        ["5.3", "roles, responsibilities", "roles and responsibilities"]),
    clause!(Iso14001, "6.1", "Actions to address risks and opportunities",
        ["6.1", "risks and opportunities"]),
    clause!(Iso14001, "6.1.2", "Environmental aspects",
        ["6.1.2", "environmental aspects"]),
    clause!(Iso14001, "6.1.3", "Compliance obligations",
        ["6.1.3", "compliance obligations"]),
    clause!(Iso14001, "6.2", "Environmental objectives and planning to achieve them",
        ["6.2", "environmental objectives"]),
    clause!(Iso14001, "7.1", "Resources",
        ["7.1", "resources"]),
    clause!(Iso14001, "7.2", "Competence",
        ["7.2", "competence"]),
    clause!(Iso14001, "7.3", "Awareness",
        ["7.3", "awareness"]),
    clause!(Iso14001, "7.4", "Communication",
        ["7.4", "communication"]),
    clause!(Iso14001, "7.5", "Documented information",
        ["7.5", "documented information"]),
    clause!(Iso14001, "8.1", "Operational planning and control",
        ["8.1", "operational planning and control"]),
    clause!(Iso14001, "8.2", "Emergency preparedness and response",
        ["8.2", "emergency preparedness"]),
    clause!(Iso14001, "9.1", "Monitoring, measurement, analysis and evaluation",
        ["9.1", "monitoring, measurement", "monitoring measurement analysis"]),
    clause!(Iso14001, "9.1.2", "Evaluation of compliance",
        ["9.1.2", "evaluation of compliance"]),
    clause!(Iso14001, "9.2", "Internal audit",
        ["9.2", "internal audit"]),
    clause!(Iso14001, "9.3", "Management review",
        ["9.3", "management review"]),
    clause!(Iso14001, "10.2", "Nonconformity and corrective action",
        ["10.2", "nonconformity and corrective action", "corrective action"]),
    clause!(Iso14001, "10.3", "Continual improvement",
        ["10.3", "continual improvement"]),

    // —— ISO 45001 ——
    clause!(Iso45001, "4.1", "Understanding the organization and its context",
        ["4.1", "context of the organization", "organisational context", "organizational context"]),
    clause!(Iso45001, "4.2", "Needs and expectations of workers and other interested parties",
        ["4.2", "interested parties", "needs and expectations of workers"]),
    clause!(Iso45001, "4.3", "Determining the scope of the OH&S management system",
        ["4.3", "scope of the oh&s", "scope of the ohs", "oh&s management system scope", "ohsms scope"]),
    clause!(Iso45001, "4.4", "OH&S management system",
        ["4.4", "oh&s management system", "ohsms", "ohs management system"]),
    clause!(Iso45001, "5.1", "Leadership and commitment",
        ["5.1", "leadership and commitment"]),
    clause!(Iso45001, "5.2", "OH&S policy",
        ["5.2", "oh&s policy", "ohs policy", "health and safety policy"]),
    clause!(Iso45001, "5.3", "Organizational roles, responsibilities and authorities",
        ["5.3", "roles, responsibilities", "roles and responsibilities"]),
    clause!(Iso45001, "5.4", "Consultation and participation of workers",
        ["5.4", "consultation and participation", "worker participation"]),
    clause!(Iso45001, "6.1", "Actions to address risks and opportunities",
        ["6.1", "risks and opportunities"]),
    clause!(Iso45001, "6.1.2", "Hazard identification and assessment of risks and opportunities",
        ["6.1.2", "hazard identification", "assessment of oh&s risks"]),
    clause!(Iso45001, "6.2", "OH&S objectives and planning to achieve them",
        ["6.2", "oh&s objectives", "ohs objectives", "health and safety objectives"]),
    clause!(Iso45001, "7.1", "Resources",
        ["7.1", "resources"]),
    clause!(Iso45001, "7.2", "Competence",
        ["7.2", "competence"]),
    clause!(Iso45001, "7.3", "Awareness",
        ["7.3", "awareness"]),
    clause!(Iso45001, "7.4", "Communication",
        ["7.4", "communication"]),
    clause!(Iso45001, "7.5", "Documented information",
        ["7.5", "documented information"]),
    clause!(Iso45001, "8.1", "Operational planning and control",
        ["8.1", "operational planning and control"]),
    clause!(Iso45001, "8.1.2", "Eliminating hazards and reducing OH&S risks",
        ["8.1.2", "eliminating hazards", "reducing oh&s risks"]),
    clause!(Iso45001, "8.2", "Emergency preparedness and response",
        ["8.2", "emergency preparedness"]),
    clause!(Iso45001, "9.1", "Monitoring, measurement, analysis and performance evaluation",
        ["9.1", "monitoring, measurement", "performance evaluation"]),
    clause!(Iso45001, "9.2", "Internal audit",
        ["9.2", "internal audit"]),
    clause!(Iso45001, "9.3", "Management review",
        ["9.3", "management review"]),
    clause!(Iso45001, "10.2", "Incident, nonconformity and corrective action",
        ["10.2", "incident, nonconformity", "nonconformity and corrective action", "corrective action"]),
    clause!(Iso45001, "10.3", "Continual improvement",
        ["10.3", "continual improvement"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_from_document_xml;
    use crate::write_minimal_docx;
    use crate::SAMPLE_STYLES;
    use std::path::PathBuf;

    #[test]
    fn parses_standard_ids() {
        assert_eq!(IsoStandard::parse("9001"), Some(IsoStandard::Iso9001));
        assert_eq!(IsoStandard::parse("ISO 45001"), Some(IsoStandard::Iso45001));
        assert_eq!(IsoStandard::parse("14001"), Some(IsoStandard::Iso14001));
        assert!(IsoStandard::parse("27001").is_none());
    }

    #[test]
    fn detects_present_and_missing_clauses() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>4.1 Context of the organization</w:t></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>5.2 Quality policy</w:t></w:r></w:p>
    <w:p><w:r><w:t>We maintain documented information for the QMS.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let text = extract_from_document_xml(xml);
        let report = evaluate_text("sample.docx", &text, &[IsoStandard::Iso9001]);
        let find = |clause: &str| {
            report
                .findings
                .iter()
                .find(|f| f.clause == clause)
                .unwrap()
        };
        assert_eq!(find("4.1").status, Presence::Present);
        assert_eq!(find("5.2").status, Presence::Present);
        assert_eq!(find("7.5").status, Presence::Present);
        assert_eq!(find("8.3").status, Presence::Missing);
        assert!(report.missing_count() > 0);
        assert!(report.present_count() >= 3);
    }

    #[test]
    fn csv_and_report_roundtrip_on_docx_fixture() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spike/fixtures-iso-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let docx = dir.join("ohsms-partial.docx");
        write_minimal_docx(
            &docx,
            "Heading1",
            "Normal",
            &[
                ("title", "4.1 Context of the organization"),
                ("title", "5.4 Consultation and participation of workers"),
                ("title", "6.1.2 Hazard identification"),
                ("body", "Emergency preparedness drills are scheduled annually."),
                ("body", "Internal audit programme covers all departments."),
            ],
            SAMPLE_STYLES,
            None,
        )
        .unwrap();

        let report = evaluate_docx(&docx, &[IsoStandard::Iso45001]).unwrap();
        assert!(report
            .findings
            .iter()
            .any(|f| f.clause == "4.1" && f.status == Presence::Present));
        assert!(report
            .findings
            .iter()
            .any(|f| f.clause == "5.4" && f.status == Presence::Present));
        assert!(report
            .findings
            .iter()
            .any(|f| f.clause == "8.2" && f.status == Presence::Present));
        assert!(report
            .findings
            .iter()
            .any(|f| f.clause == "9.2" && f.status == Presence::Present));
        assert!(report
            .findings
            .iter()
            .any(|f| f.clause == "8.1.2" && f.status == Presence::Missing));

        let (csv_path, txt_path) = write_exports(&report, &dir).unwrap();
        let csv = std::fs::read_to_string(csv_path).unwrap();
        let txt = std::fs::read_to_string(txt_path).unwrap();
        assert!(csv.contains("document,standard,clause,title,status,matched_pattern"));
        assert!(csv.contains("45001"));
        assert!(txt.contains("ISO 45001"));
        assert!(txt.contains("[PASS]") || txt.contains("[MISS]"));
    }
}
