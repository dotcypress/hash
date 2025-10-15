use chrono::Utc;
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub const SCRIPT_SUFFIX: &str = ".ha.sh";
pub const MAX_SCRIPT_SIZE: u64 = 655_360;

#[derive(Debug)]
pub enum Error {
    IO(io::Error),
    ScriptNotFound(PathBuf),
    UnsupportedScript(PathBuf),
    TransformFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::IO(err) => write!(f, "IO: {}", err),
            Self::TransformFailed => write!(f, "Transform failed"),
            Self::ScriptNotFound(path) => write!(f, "Script not found: {:?}", path),
            Self::UnsupportedScript(path) => write!(f, "Unsupported script: {:?}", path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Script {
    path: PathBuf,
}

impl Script {
    pub fn from_file(path: &Path) -> Result<Self, Error> {
        if path
            .to_str()
            .map(|p| !p.ends_with(SCRIPT_SUFFIX))
            .unwrap_or_default()
        {
            Err(Error::UnsupportedScript(path.to_path_buf()))
        } else if !path.is_file() {
            Err(Error::ScriptNotFound(path.to_path_buf()))
        } else {
            let path = path.canonicalize().map_err(Error::IO)?;
            Ok(Self { path })
        }
    }

    pub fn parent(&self) -> Result<&Path, Error> {
        self.path
            .parent()
            .ok_or(Error::ScriptNotFound(self.path.to_path_buf()))
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .and_then(|p| p.to_str())
            .map(|p| p.replace(SCRIPT_SUFFIX, ""))
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Runner {
    host_id: String,
    decoder: Option<String>,
    encoder: Option<String>,
}

impl Runner {
    pub fn new(host_id: String, decoder: Option<String>, encoder: Option<String>) -> Self {
        Self {
            host_id,
            decoder,
            encoder,
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start(&self, path: &Path) -> Result<(), Error> {
        if path.is_file() {
            self.eval_script(path, true)
        } else {
            self.eval_dir(path, true)
        }
    }

    #[cfg(target_os = "linux")]
    pub fn start(&self, path: &Path, watch: bool) -> Result<(), Error> {
        if path.is_file() {
            self.eval_script(path, true)
        } else {
            if !watch {
                self.eval_dir(path, true)?;
            } else {
                use mount_watcher::{MountWatcher, WatchControl};
                use std::{sync::mpsc, time::Instant};

                let mount_point = path.to_str().unwrap_or_default().to_owned();
                let (tx, rx) = mpsc::channel::<Instant>();

                let _ = MountWatcher::new(move |ev| {
                    if ev.mounted.iter().any(|m| m.mount_point == mount_point) {
                        tx.send(Instant::now()).ok();
                    }
                    WatchControl::Continue
                });

                let mut last_ts = None;
                for ts in rx {
                    last_ts = match last_ts {
                        Some(last_ts) if ts.duration_since(last_ts).as_secs() > 1 => Some(ts),
                        None => Some(ts),
                        _ => continue,
                    };
                    self.eval_dir(path, false).ok();
                }
            }
            Ok(())
        }
    }

    fn eval_dir(&self, dir: &Path, wait: bool) -> Result<(), Error> {
        let files = fs::read_dir(dir).map_err(Error::IO)?;
        let files: Vec<PathBuf> = files
            .filter_map(|e| e.ok())
            .filter_map(|file| {
                if let Some(file_name) = file
                    .path()
                    .file_name()
                    .and_then(|file_name| file_name.to_str())
                    && file_name.starts_with(".")
                {
                    return None;
                }
                Some(file.path())
            })
            .collect();

        for file in files {
            if let Err(err) = self.eval_script(&file, wait) {
                eprintln!("Script evaluation error: {}", err);
            }
        }

        Ok(())
    }

    fn eval_script(&self, path: &Path, wait: bool) -> Result<(), Error> {
        let script = Script::from_file(path)?;
        let mut run_dir = script.parent()?.to_path_buf();
        run_dir.push(format!(
            "{}-run-{}",
            script.name(),
            Utc::now().format("%Y-%m-%d-%H-%M-%S")
        ));
        fs::create_dir(&run_dir).map_err(Error::IO)?;

        if let Err(err) = self.run(&script, &run_dir, wait) {
            run_dir.push("error.log");
            fs::write(run_dir, format!("{}", err))
                .map_err(Error::IO)
                .ok();
        }

        Ok(())
    }

    fn run(&self, script: &Script, run_dir: &Path, wait: bool) -> Result<(), Error> {
        let script_len = fs::metadata(&script.path).map_err(Error::IO)?.len();
        if script_len > MAX_SCRIPT_SIZE {
            return Err(Error::UnsupportedScript(script.path.to_path_buf()));
        }

        let mut buf = Vec::new();
        let script_file = File::open(&script.path).map_err(Error::IO)?;
        self.transform(script_file, &mut buf, &self.decoder)?;

        let script_text = str::from_utf8(&buf)
            .map(|s| s.to_owned())
            .map_err(|_| Error::UnsupportedScript(script.path.to_path_buf()))?;

        let envs = [
            ("HASH_HOST", self.host_id.clone()),
            ("HASH_DECODER", self.decoder.clone().unwrap_or_default()),
            ("HASH_ENCODER", self.encoder.clone().unwrap_or_default()),
            ("HASH_SCRIPT", script.name()),
            (
                "HASH_RUN_DIR",
                run_dir.to_str().unwrap_or_default().to_owned(),
            ),
        ];

        if wait {
            let Output { stdout, stderr, .. } = Command::new("sh")
                .envs(envs)
                .args(["-c", &script_text])
                .current_dir(script.parent()?)
                .output()
                .map_err(Error::IO)?;

            if !stdout.is_empty() {
                let mut path = run_dir.to_path_buf();
                path.push("stdout.log");
                let mut log = File::create(path).map_err(Error::IO)?;
                self.transform(&stdout[..], &mut log, &self.encoder)?;
                log.flush().map_err(Error::IO)?;
            }

            if !stderr.is_empty() {
                let mut path = run_dir.to_path_buf();
                path.push("stderr.log");
                let mut log = File::create(path).map_err(Error::IO)?;
                self.transform(&stderr[..], &mut log, &self.encoder)?;
                log.flush().map_err(Error::IO)?;
            }
        } else {
            Command::new("sh")
                .envs(envs)
                .args(["-c", &script_text])
                .current_dir(script.parent()?)
                .spawn()
                .map_err(Error::IO)?;
        }

        Ok(())
    }

    fn transform(
        &self,
        mut reader: impl Read,
        mut writer: impl Write,
        transformer: &Option<String>,
    ) -> Result<(), Error> {
        match transformer {
            None => {
                io::copy(&mut reader, &mut writer).map_err(Error::IO)?;
            }
            Some(transform) => {
                let mut cmd = Command::new("sh")
                    .args(["-c", transform])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(Error::IO)?;

                if let Some(stdin) = &mut cmd.stdin {
                    io::copy(&mut reader, stdin).map_err(Error::IO)?;
                }

                let result = cmd.wait().map_err(Error::IO)?;
                if result.success() {
                    if let Some(mut stdout) = cmd.stdout {
                        io::copy(&mut stdout, &mut writer).map_err(Error::IO)?;
                    }
                } else {
                    return Err(Error::TransformFailed);
                }
            }
        }
        Ok(())
    }
}
