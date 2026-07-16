use std::{
  collections::{BTreeMap, BTreeSet},
  path::Path,
  process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfMetadataEntry {
  pub group: String,
  pub name: String,
  pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataChange {
  pub tag: String,
  pub old_value: Option<String>,
  pub new_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataEdit {
  pub tags: Vec<MetadataChange>,
}

impl MetadataEdit {
  pub fn is_empty(&self) -> bool {
    self.tags.is_empty()
  }

  pub fn change_count(&self) -> usize {
    self.tags.len()
  }
}

const EDITABLE_TAGS: &[&str] = &[
  "Title",
  "Author",
  "Subject",
  "Keywords",
  "Creator",
  "Producer",
  "CreationDate",
  "ModifyDate",
];

pub fn read_pdf_metadata(path: &Path) -> Result<Vec<PdfMetadataEntry>, String> {
  let output = Command::new("exiftool")
    .args(["-G1", "-s"])
    .arg(path)
    .output()
    .map_err(|err| format!("failed to run exiftool; install exiftool or put it in PATH: {err}"))?;
  if !output.status.success() {
    return Err(format!(
      "exiftool failed: {}{}",
      String::from_utf8_lossy(&output.stderr).trim(),
      String::from_utf8_lossy(&output.stdout).trim()
    ));
  }
  parse_exiftool_metadata(&String::from_utf8_lossy(&output.stdout))
}

pub fn metadata_edit_draft(path: &Path, entries: &[PdfMetadataEntry]) -> String {
  let values = editable_metadata_map(entries);
  let mut out = String::new();
  out.push_str("# Edit PDF document metadata. Save and exit to continue.\n");
  out.push_str("# pdf-tui will ask for confirmation before writing changes.\n");
  out.push_str("# Empty strings clear the corresponding PDF metadata field.\n");
  out.push_str(&format!("# file = {:?}\n\n", path.display().to_string()));
  out.push_str("[metadata]\n");
  for tag in EDITABLE_TAGS {
    let value = values.get(*tag).cloned().unwrap_or_default();
    out.push_str(&format!("{} = {}\n", toml_key(tag), toml_string(&value)));
  }
  out
}

pub fn metadata_changes_from_edit(
  original: &[PdfMetadataEntry],
  edited: &str,
) -> Result<MetadataEdit, String> {
  let original = editable_metadata_map(original);
  let value = toml::from_str::<toml::Value>(edited)
    .map_err(|err| format!("metadata draft is not valid TOML: {err}"))?;
  let Some(metadata) = value.get("metadata").and_then(toml::Value::as_table) else {
    return Err("metadata draft must contain a [metadata] table".to_string());
  };

  let editable = editable_tag_set();
  let mut edit = MetadataEdit::default();
  for (tag, value) in metadata {
    if !editable.contains(tag.as_str()) {
      return Err(format!("unsupported PDF metadata tag: {tag}"));
    }
    let Some(value) = value.as_str() else {
      return Err(format!("metadata tag {tag} must be a string"));
    };
    let old_value = original.get(tag).cloned();
    if old_value.as_deref().unwrap_or_default() != value {
      edit.tags.push(MetadataChange {
        tag: tag.clone(),
        old_value,
        new_value: value.to_string(),
      });
    }
  }
  edit.tags.sort_by(|left, right| left.tag.cmp(&right.tag));
  Ok(edit)
}

pub fn write_pdf_metadata_with_exiftool(
  path: &Path,
  changes: &[MetadataChange],
) -> Result<(), String> {
  if changes.is_empty() {
    return Ok(());
  }
  let mut command = Command::new("exiftool");
  command.arg("-overwrite_original");
  for change in changes {
    command.arg(format!("-{}={}", change.tag, change.new_value));
  }
  command.arg(path);
  let output = command
    .output()
    .map_err(|err| format!("failed to run exiftool; install exiftool or put it in PATH: {err}"))?;
  if output.status.success() {
    Ok(())
  } else {
    Err(format!(
      "exiftool failed: {}{}",
      String::from_utf8_lossy(&output.stderr).trim(),
      String::from_utf8_lossy(&output.stdout).trim()
    ))
  }
}

fn parse_exiftool_metadata(output: &str) -> Result<Vec<PdfMetadataEntry>, String> {
  let mut entries = Vec::new();
  for line in output.lines() {
    let Some(rest) = line.strip_prefix('[') else {
      continue;
    };
    let Some((group, rest)) = rest.split_once(']') else {
      continue;
    };
    let Some((name, value)) = rest.split_once(':') else {
      continue;
    };
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.is_empty() {
      continue;
    }
    entries.push(PdfMetadataEntry {
      group: group.trim().to_string(),
      name: name.to_string(),
      value: value.to_string(),
    });
  }
  entries.sort_by(|left, right| {
    left
      .group
      .cmp(&right.group)
      .then_with(|| left.name.cmp(&right.name))
  });
  Ok(entries)
}

fn editable_metadata_map(entries: &[PdfMetadataEntry]) -> BTreeMap<String, String> {
  let editable = editable_tag_set();
  entries
    .iter()
    .filter(|entry| editable.contains(entry.name.as_str()))
    .map(|entry| (entry.name.clone(), entry.value.clone()))
    .collect()
}

fn editable_tag_set() -> BTreeSet<&'static str> {
  EDITABLE_TAGS.iter().copied().collect()
}

fn toml_key(value: &str) -> String {
  toml_string(value)
}

fn toml_string(value: &str) -> String {
  let mut out = String::with_capacity(value.len() + 2);
  out.push('"');
  for ch in value.chars() {
    match ch {
      '\\' => out.push_str("\\\\"),
      '"' => out.push_str("\\\""),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      ch if ch.is_control() => out.push(' '),
      ch => out.push(ch),
    }
  }
  out.push('"');
  out
}
