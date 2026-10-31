use std::path::PathBuf;

use serde_json::Value;

use super::models::{Extraction, Verification};
use super::{ExtractOptions, VerifyOptions};
use crate::client::OfflineSettings;
use crate::error::{Error, Result};
use crate::files::{check_id, expand, File};
use crate::offline::{on_disk, path_arg, write_private};
use crate::Policy;

pub(super) async fn verify(o: &OfflineSettings, file: File, opts: VerifyOptions) -> Result<Verification> {
    let runner = o.nib()?;
    let scratch = runner.scratch()?;
    let tmp = scratch.path();
    let questioned = on_disk(&file, tmp, "questioned").await?;
    let mut given = expand(opts.references)?;
    if let Some(subject) = &opts.subject {
        check_id(subject, "subject")?;
        let source = o.reference_source.as_ref().ok_or_else(|| {
            Error::not_supported(
                "offline, a subject needs a reference source on the client; or pass the references",
            )
        })?;
        let found = source.find(subject)?;
        if found.is_empty() {
            return Err(Error::no_references(format!(
                "no reference signatures for {subject}"
            )));
        }
        given.extend(found);
    }
    if given.is_empty() {
        return Err(Error::no_references("no reference signatures to compare with"));
    }
    let mut args = vec!["verify".to_string(), path_arg(&questioned)];
    for (i, f) in given.iter().enumerate() {
        let p = on_disk(f, tmp, &format!("reference-{}", i + 1)).await?;
        args.push("--ref".into());
        args.push(path_arg(&p));
    }
    push_form_args(&mut args, o, &opts.target, &opts.template_id);
    if let Some(p) = opts.policy {
        let path = match p {
            Policy::File(p) => p,
            Policy::Json(v) => {
                let path = tmp.join("policy.json");
                write_private(&path, v.to_string().as_bytes()).await?;
                path
            }
            Policy::Named(_) => {
                return Err(Error::invalid(
                    "offline, the policy is a JSON file or value; saved policies live in your Unforged account",
                ))
            }
        };
        args.push("--policy".into());
        args.push(path_arg(&path));
    }
    if o.auto_approve {
        args.push("--auto-approve".into());
    }
    if let Some(dir) = &opts.save_crops {
        args.push("--save-crops".into());
        args.push(path_arg(dir));
    }
    let out = runner.run(args, opts.timeout, &[0]).await?;
    drop(scratch);
    let v: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| Error::Offline {
        message: "the offline binary's answer wasn't JSON".into(),
        exit_code: Some(0),
        stderr: String::new(),
    })?;
    let mut r = Verification::from_offline(&v, out.notices);
    r.subject_reference = opts.subject;
    r.reference = opts.reference;
    r.process = opts.process;
    r.metadata = opts.metadata;
    Ok(r)
}

fn push_form_args(
    args: &mut Vec<String>,
    o: &OfflineSettings,
    target: &Option<crate::Target>,
    template_id: &Option<String>,
) {
    if let Some(t) = target {
        args.push("--target".into());
        args.push(t.to_arg());
    }
    for t in &o.templates {
        args.push("--template".into());
        args.push(path_arg(t));
    }
    if let Some(id) = template_id {
        args.push("--template-id".into());
        args.push(id.clone());
    }
}

pub(super) async fn extract(
    o: &OfflineSettings,
    files: Vec<File>,
    out: PathBuf,
    opts: ExtractOptions,
) -> Result<Vec<Extraction>> {
    if files.is_empty() {
        return Err(Error::invalid("there are no files to look in"));
    }
    let runner = o.nib()?;
    let scratch = runner.scratch()?;
    let mut args = vec!["extract-signature".to_string()];
    for (i, f) in files.iter().enumerate() {
        let p = on_disk(f, scratch.path(), &format!("document-{}", i + 1)).await?;
        args.push(path_arg(&p));
    }
    args.push("--out".into());
    args.push(path_arg(&out));
    push_form_args(&mut args, o, &opts.target, &opts.template_id);
    runner.run(args, opts.timeout, &[0, 1]).await?;
    drop(scratch);
    let manifest = tokio::fs::read(out.join("manifest.json")).await?;
    let v: Value = serde_json::from_slice(&manifest)
        .map_err(|e| Error::Decode(format!("the extraction's manifest.json can't be read: {e}")))?;
    Ok(v.as_array()
        .map(|a| a.iter().map(|e| Extraction::from_json(e, &out)).collect())
        .unwrap_or_default())
}
