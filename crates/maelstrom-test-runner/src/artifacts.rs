use crate::ClientTrait;
use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt as _, WriteBytesExt as _};
use maelstrom_base::Sha256Digest;
use maelstrom_client::spec::{Layer, PrefixOptions};
use maelstrom_util::elf::read_shared_libraries;
use maelstrom_util::fs::Fs;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::{
    io,
    path::{Path, PathBuf},
};

fn so_listing_path_from_binary_path(path: &Path) -> PathBuf {
    let mut path = path.to_owned();
    path.set_extension("so_listing");
    path
}

fn check_for_cached_so_listing(fs: &Fs, binary_path: &Path) -> Result<Option<Vec<PathBuf>>> {
    let listing_path = so_listing_path_from_binary_path(binary_path);
    if fs.exists(&listing_path) {
        let listing_mtime = fs.metadata(&listing_path)?.modified()?;
        let binary_mtime = fs.metadata(binary_path)?.modified()?;
        if binary_mtime < listing_mtime {
            return Ok(Some(decode_paths(fs.open_file(listing_path)?)?));
        }
    }
    Ok(None)
}

fn encode_paths(paths: &[PathBuf], mut out: impl io::Write) -> Result<()> {
    out.write_u64::<BigEndian>(paths.len() as u64)?;
    for path in paths {
        let s = path.as_os_str();
        out.write_u64::<BigEndian>(s.len() as u64)?;
        out.write_all(s.as_encoded_bytes())?;
    }
    Ok(())
}

fn decode_paths(mut input: impl io::Read) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    let num_paths = input.read_u64::<BigEndian>()?;
    for _ in 0..num_paths {
        let path_len = input.read_u64::<BigEndian>()?;
        let mut buffer = vec![0; path_len as usize];
        input.read_exact(&mut buffer)?;
        paths.push(OsString::from_vec(buffer).into());
    }

    let extra = std::io::copy(&mut input, &mut std::io::sink())?;
    if extra > 0 {
        bail!("unknown trailing data")
    }

    Ok(paths)
}

fn create_artifact_for_binary(binary_path: &Path, log: slog::Logger) -> Result<Layer> {
    let mut manifest_path = PathBuf::from(binary_path);
    assert!(manifest_path.set_extension("manifest"));

    slog::debug!(log, "adding layer for binary"; "binary" => ?binary_path);
    Ok(Layer::Paths {
        paths: vec![binary_path.to_path_buf().try_into()?],
        prefix_options: PrefixOptions {
            strip_prefix: Some(binary_path.parent().unwrap().to_path_buf().try_into()?),
            ..Default::default()
        },
    })
}

fn create_artifact_for_binary_deps(binary_path: &Path, log: slog::Logger) -> Result<Layer> {
    let fs = Fs::new();

    let paths = if let Some(paths) = check_for_cached_so_listing(&fs, binary_path)? {
        slog::debug!(log, "found cached shared libraries"; "path" => ?binary_path);
        paths
    } else {
        slog::debug!(log, "reading shared libraries"; "path" => ?binary_path);
        let paths = read_shared_libraries(binary_path)?;
        encode_paths(
            &paths,
            fs.create_file(so_listing_path_from_binary_path(binary_path))?,
        )?;
        paths
    };

    slog::debug!(log, "adding layer for binary deps"; "binary" => ?binary_path);
    Ok(Layer::Paths {
        paths: paths
            .into_iter()
            .map(|p| p.try_into())
            .collect::<std::result::Result<_, _>>()?,
        prefix_options: PrefixOptions {
            strip_prefix: Some("/".into()),
            follow_symlinks: true,
            ..Default::default()
        },
    })
}

#[derive(Clone)]
pub struct GeneratedArtifacts {
    pub binary: Sha256Digest,
    pub deps: Sha256Digest,
}

pub fn add_generated_artifacts(
    client: &impl ClientTrait,
    binary_path: &Path,
    log: slog::Logger,
) -> Result<GeneratedArtifacts> {
    let (binary_artifact, _) =
        client.add_layer(create_artifact_for_binary(binary_path, log.clone())?)?;
    let (deps_artifact, _) =
        client.add_layer(create_artifact_for_binary_deps(binary_path, log)?)?;
    Ok(GeneratedArtifacts {
        binary: binary_artifact,
        deps: deps_artifact,
    })
}
