use chrono::Utc;
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

pub const SCRIPT_SUFFIX: &str = ".ha.sh";
pub const MAX_SCRIPT_SIZE: u64 = 655_360;

#[derive(Debug)]
pub enum Error {
    IO(io::Error),
    ScriptNotFound(PathBuf),
    UnsupportedScript(PathBuf),
    DecodeFailed(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::IO(err) => write!(f, "IO: {}", err),
            Self::DecodeFailed(path) => write!(f, "Script decode failed: {:?}", path),
            Self::ScriptNotFound(path) => write!(f, "Script not found: {:?}", path),
            Self::UnsupportedScript(path) => write!(f, "Unsupported script: {:?}", path),
        }
    }
}

#[derive(Debug)]
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
        } else if path.is_dir() {
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

    pub fn path(&self) -> String {
        self.path.to_str().unwrap_or_default().to_owned()
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .and_then(|f| f.to_str().to_owned())
            .map(|f| f.to_owned())
            .unwrap_or_default()
            .replace(SCRIPT_SUFFIX, "")
    }
}

#[derive(Debug)]
pub struct Runner {
    host_id: String,
    decoder: Option<String>,
}

impl Runner {
    pub fn new(host_id: String, decoder: Option<String>) -> Self {
        Self { host_id, decoder }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start(&self, path: &Path) -> Result<(), Error> {
        if path.is_file() {
            self.eval_script(path)
        } else {
            self.eval_dir(path)
        }
    }

    #[cfg(target_os = "linux")]
    pub fn start(&self, path: &Path, watch: bool) -> Result<(), Error> {
        if path.is_file() {
            self.eval_script(path)
        } else {
            if !watch {
                self.eval_dir(path)?;
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
                    self.eval_dir(path).ok();
                }
            }
            Ok(())
        }
    }

    fn eval_dir(&self, dir: &Path) -> Result<(), Error> {
        let files = fs::read_dir(dir).map_err(Error::IO)?;
        let files: Vec<PathBuf> = files
            .filter_map(|f| f.ok())
            .filter_map(|file| {
                if let Ok(file_type) = file.file_type()
                    && file_type.is_dir()
                {
                    return None;
                } else if let Some(file_name) = file
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
            if let Err(err) = self.eval_script(&file) {
                eprintln!("Script evaluation error: {}", err);
            }
        }

        Ok(())
    }

    fn eval_script(&self, path: &Path) -> Result<(), Error> {
        let script = Script::from_file(path)?;
        let mut run_dir = script.parent()?.to_path_buf();
        run_dir.push(format!(
            "{}-run-{}",
            script.name(),
            Utc::now().format("%Y-%m-%d-%H-%M-%S")
        ));
        fs::create_dir(&run_dir).map_err(Error::IO)?;

        if let Err(err) = self.spawn(&script, &run_dir) {
            run_dir.push("error.log");
            fs::write(run_dir, format!("{}", err))
                .map_err(Error::IO)
                .ok();
        }

        Ok(())
    }

    fn spawn(&self, script: &Script, run_dir: &Path) -> Result<Child, Error> {
        let script_len = fs::metadata(&script.path).map_err(Error::IO)?.len();
        if script_len > MAX_SCRIPT_SIZE {
            return Err(Error::UnsupportedScript(script.path.to_path_buf()));
        }

        let mut script_file = fs::File::open(&script.path).map_err(Error::IO)?;
        let mut buf = Vec::new();

        match &self.decoder {
            None => {
                io::copy(&mut script_file, &mut buf).map_err(Error::IO)?;
            }
            Some(transform) => {
                let mut cmd = Command::new("sh")
                    .args(["-c", transform])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .map_err(Error::IO)?;

                if let Some(stdin) = &mut cmd.stdin {
                    io::copy(&mut script_file, stdin).map_err(Error::IO)?;
                }

                let result = cmd.wait().map_err(Error::IO)?;
                if result.success() {
                    if let Some(mut stdout) = cmd.stdout {
                        io::copy(&mut stdout, &mut buf).map_err(Error::IO)?;
                    }
                } else {
                    return Err(Error::DecodeFailed(script.path.to_path_buf()));
                }
            }
        }

        let script_text = str::from_utf8(&buf)
            .map(|s| s.to_owned())
            .map_err(|_| Error::UnsupportedScript(script.path.to_path_buf()))?;
        let run_dir = run_dir.to_str().unwrap_or_default().to_owned();
        let decoder = self.decoder.clone().unwrap_or("cat".to_owned());

        Command::new("sh")
            .args(["-c", &script_text])
            .current_dir(run_dir)
            .env("HASH_SCRIPT", script.path())
            .env("HASH_HOST", &self.host_id)
            .env("HASH_DECODER", &decoder)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Error::IO)
    }
}
