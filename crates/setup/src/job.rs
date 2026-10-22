//! The install job the setup UI starts: the disks service, then the installer,
//! as the same programs init runs at boot. On success the installer hands over
//! to the boot service, which reboots into the new slot.

use std::process::Command;
use std::sync::Mutex;

use neslina_common::{error, info};
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Job {
    Idle,
    Running {
        step: &'static str,
    },
    Failed {
        step: &'static str,
        reason: String,
    },
    /// Every step exited cleanly but the host did not reboot into a new slot:
    /// a step found nothing to do, and the kernel log says why.
    Finished,
}

pub struct Runner {
    job: Mutex<Job>,
}

impl Default for Runner {
    fn default() -> Self {
        Self { job: Mutex::new(Job::Idle) }
    }
}

const STEPS: &[(&str, &str)] =
    &[("disks", "/usr/lib/neslina/neslina-disks"), ("install", "/usr/lib/neslina/neslina-install")];

impl Runner {
    pub fn current(&self) -> Job {
        self.job.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, job: Job) {
        *self.job.lock().unwrap_or_else(|e| e.into_inner()) = job;
    }

    /// Starts the job unless one is running. Returns whether it started.
    pub fn start(&'static self) -> bool {
        {
            let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*job, Job::Running { .. }) {
                return false;
            }
            *job = Job::Running { step: STEPS[0].0 };
        }
        std::thread::spawn(move || {
            for (step, path) in STEPS {
                self.set(Job::Running { step });
                info!("running {step}");
                match Command::new(path).status() {
                    Ok(s) if s.success() => {}
                    Ok(s) => {
                        error!("{step} failed ({s})");
                        self.set(Job::Failed { step, reason: s.to_string() });
                        return;
                    }
                    Err(e) => {
                        error!("starting {step}: {e}");
                        self.set(Job::Failed { step, reason: e.to_string() });
                        return;
                    }
                }
            }
            // The installer reboots the host when it succeeds; reaching here
            // means a step found nothing to do.
            self.set(Job::Finished);
        });
        true
    }
}
