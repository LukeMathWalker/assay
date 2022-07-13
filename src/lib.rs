/*
 * Copyright (C) 2021 Michael Gattozzi <self@mgattozzi.dev>
 *
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! > as·say /ˈaˌsā,aˈsā/ noun - the testing of a metal or ore to determine its ingredients and quality.
//!
//! `assay` is a super powered testing macro for Rust. It lets you run test in
//! parallel while also being their own process so that you can set env vars, or
//! do other per process kinds of settings without interfering with each other,
//! auto mounting and changing to a tempdir, including files in it, choosing
//! setup and tear down functions, async tests, and more!
//!
#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/HOW_TO_USE.md"))]

pub use assay_proc_macro::assay;
#[doc(hidden)]
pub use pretty_assertions_sorted::{assert_eq, assert_eq_sorted, assert_ne};
#[doc(hidden)]
pub use rusty_fork::{fork, rusty_fork_id, rusty_fork_test_name, ChildWrapper};

use anyhow::Context;
use fs_err::create_dir_all;
use std::{
  env,
  error::Error,
  fs::copy,
  path::{Path, PathBuf},
};
use tempfile::{Builder, TempDir};

enum TestWorkingDirectory {
  Temporary(TempDir),
  Rooted(PathBuf),
}

impl TestWorkingDirectory {
  fn path(&self) -> &Path {
    match self {
      TestWorkingDirectory::Temporary(d) => d.path(),
      TestWorkingDirectory::Rooted(p) => p.as_path(),
    }
  }
}

#[doc(hidden)]
pub struct PrivateFS {
  ran_from: PathBuf,
  directory: TestWorkingDirectory,
}

impl PrivateFS {
  pub fn temporary() -> Result<Self, Box<dyn Error>> {
    let ran_from = env::current_dir()?;
    let directory = Builder::new().prefix("private").tempdir()?;
    env::set_current_dir(directory.path())?;
    Ok(Self {
      ran_from,
      directory: TestWorkingDirectory::Temporary(directory),
    })
  }

  pub fn rooted(root: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
    let ran_from = env::current_dir()?;
    let root = root.as_ref();
    fs_err::create_dir_all(root).context("Failed to create the specific `root_directory`")?;
    env::set_current_dir(root)
      .context("Failed to set test current directory to the specified `root_directory`")?;
    Ok(Self {
      ran_from,
      directory: TestWorkingDirectory::Rooted(root.to_path_buf()),
    })
  }

  pub fn include<S, D>(
    &self,
    source_path: S,
    destination_path: Option<D>,
  ) -> Result<(), Box<dyn Error>>
  where
    S: AsRef<Path>,
    D: AsRef<Path>,
  {
    // Get our pathbuf to the file/directory to include
    let inner_path = {
      let mut p = source_path.as_ref().to_owned();
      // If the path given is not absolute then it's relative to the dir we
      // ran the test from
      let is_relative = p.is_relative();
      if is_relative {
        p = self.ran_from.join(&source_path);
      }
      p
    };

    let destination_path = destination_path.map(|p| p.as_ref().to_owned());

    if inner_path.is_file() {
      self.include_file(inner_path, &destination_path)?;
    } else if inner_path.is_dir() {
      self.include_directory(inner_path, &destination_path)?;
    } else {
      panic!(
        "The source path passed to `#[include()]` must point to a file or a directory. {:?} is neither.",
        inner_path
      );
    }
    Ok(())
  }

  fn include_file(
    &self,
    inner_path: PathBuf,
    destination_path: &Option<PathBuf>,
  ) -> Result<(), Box<dyn Error>> {
    // Get our working directory
    let dir = self.directory.path().to_owned();

    let destination_path = match destination_path {
      None => {
        // If the destination path is unspecified, we mount the file in the root directory
        // of the test's private filesystem
        match inner_path.file_name() {
          Some(filename) => dir.join(filename),
          None => {
            panic!(
              "Failed to extract the filename from the source path, {:?}.",
              inner_path
            )
          }
        }
      }
      Some(p) => {
        if !p.is_relative() {
          panic!(
            "The destination path for included files must be a relative path. {:?} isn't.",
            p
          );
        }
        // If the relative path to the file includes parent directories create
        // them
        if let Some(parent) = p.parent() {
          create_dir_all(dir.join(parent)).context("Failed to create the parent directory for a file that should have been copied into the test working directory")?;
        }
        dir.join(p)
      }
    };

    // Copy the file over from the file system into the temp file system
    copy(inner_path, destination_path)
      .context("Failed to copy a file into the test working directory")?;
    Ok(())
  }

  fn include_directory(
    &self,
    inner_path: PathBuf,
    destination_path: &Option<PathBuf>,
  ) -> Result<(), Box<dyn Error>> {
    // Get our working directory
    let dir = self.directory.path().to_owned();

    let destination_path = match destination_path {
      // If the destination path is unspecified, we mount the contents of the directory
      // in the root directory of the test's private filesystem
      None => dir,
      Some(p) => {
        if !p.is_relative() {
          panic!(
            "The destination path for the included directory must be a relative path. {:?} isn't.",
            p
          );
        }
        // If the relative path to the file includes parent directories create them
        if let Some(parent) = p.parent() {
          create_dir_all(dir.join(parent))
            .context("Failed to create the parent directory for a directory that will be included into the test working directory")?;
        }
        dir.join(p)
      }
    };

    let mut o = fs_extra::dir::CopyOptions::new();
    o.content_only = true;
    // Copy the file over from the file system into the temp file system
    fs_extra::dir::copy(inner_path, destination_path, &o)
      .context("Failed to copy the content of a directory into the test working directory")?;
    Ok(())
  }
}

// Async functionality
#[doc(hidden)]
#[cfg(any(feature = "async-tokio-runtime", feature = "async-std-runtime"))]
pub mod async_runtime {
  use std::{error::Error, future::Future};
  pub struct Runtime;
  impl Runtime {
    #[cfg(feature = "async-tokio-runtime")]
    pub fn block_on<F: Future>(fut: F) -> Result<F::Output, Box<dyn Error>> {
      Ok(tokio::runtime::Runtime::new()?.block_on(fut))
    }
    #[cfg(feature = "async-std-runtime")]
    pub fn block_on<F: Future>(fut: F) -> Result<F::Output, Box<dyn Error>> {
      Ok(async_std::task::block_on(fut))
    }
  }
}
