use std::{path::Path, process::Stdio, time::Duration};

use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

const CLIPBOARD_WAIT_GRACE: Duration = Duration::from_millis(500);

pub async fn copy_text(text: String) -> Result<(), String> {
  if text.is_empty() {
    return Err("selection contains no embedded text".to_string());
  }
  if cfg!(target_os = "macos") {
    return write_stdin_to_command("pbcopy", &[], text.into_bytes()).await;
  }

  let bytes = text.into_bytes();
  let mut errors = Vec::new();
  for (program, args) in [
    ("wl-copy", &[][..]),
    ("xclip", &["-selection", "clipboard"][..]),
    ("xsel", &["--clipboard", "--input"][..]),
  ] {
    match write_stdin_to_command(program, args, bytes.clone()).await {
      Ok(()) => return Ok(()),
      Err(error) => errors.push(error),
    }
  }
  Err(format!("failed to copy text: {}", errors.join("; ")))
}

pub async fn copy_png(path: &Path) -> Result<(), String> {
  let bytes = tokio::fs::read(path)
    .await
    .map_err(|error| format!("failed to read {}: {error}", path.display()))?;

  if cfg!(target_os = "macos") {
    return copy_png_macos(path).await;
  }

  let mut errors = Vec::new();
  for (program, args) in [
    ("wl-copy", &["--type", "image/png"][..]),
    ("xclip", &["-selection", "clipboard", "-t", "image/png"][..]),
  ] {
    match write_stdin_to_command(program, args, bytes.clone()).await {
      Ok(()) => return Ok(()),
      Err(error) => errors.push(error),
    }
  }
  Err(format!("failed to copy image: {}", errors.join("; ")))
}

async fn copy_png_macos(path: &Path) -> Result<(), String> {
  let class_open = char::from_u32(0x00ab).unwrap_or('<');
  let class_close = char::from_u32(0x00bb).unwrap_or('>');
  let script = format!(
    "set the clipboard to (read (POSIX file \"{}\") as {class_open}class PNGf{class_close})",
    path.display().to_string().replace('"', "\\\"")
  );
  let output = Command::new("osascript")
    .arg("-e")
    .arg(script)
    .output()
    .await
    .map_err(|error| format!("failed to run osascript: {error}"))?;
  if output.status.success() {
    Ok(())
  } else {
    Err(command_error("osascript", output.stderr, output.stdout))
  }
}

async fn write_stdin_to_command(
  program: &str,
  args: &[&str],
  bytes: Vec<u8>,
) -> Result<(), String> {
  let mut child = Command::new(program)
    .args(args)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|error| format!("{program}: {error}"))?;
  let Some(mut stdin) = child.stdin.take() else {
    return Err(format!("{program}: stdin unavailable"));
  };
  stdin
    .write_all(&bytes)
    .await
    .map_err(|error| format!("{program}: failed to write stdin: {error}"))?;
  drop(stdin);

  match timeout(CLIPBOARD_WAIT_GRACE, child.wait()).await {
    Ok(Ok(status)) if status.success() => Ok(()),
    Ok(Ok(status)) => Err(format!("{program}: exited with {status}")),
    Ok(Err(error)) => Err(format!("{program}: failed to wait: {error}")),
    Err(_) => {
      tokio::spawn(async move {
        let _ = child.wait().await;
      });
      Ok(())
    }
  }
}

fn command_error(program: &str, stderr: Vec<u8>, stdout: Vec<u8>) -> String {
  let stderr = String::from_utf8_lossy(&stderr);
  let stdout = String::from_utf8_lossy(&stdout);
  let message = format!("{}{}", stderr.trim(), stdout.trim());
  if message.is_empty() {
    format!("{program}: exited unsuccessfully")
  } else {
    format!("{program}: {message}")
  }
}
