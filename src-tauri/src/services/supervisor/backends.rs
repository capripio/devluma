//! Supervisor support for sites: on-demand PHP FastCGI backends, config validation and
//! reloads of a web server.

use std::path::PathBuf;
use std::sync::Arc;

use super::behavior::{Failure, failure};
use super::{Supervisor, db_fail};
use crate::domain::adapter::AdapterId;
use crate::domain::service::ServiceState;
use crate::error::ErrorCode;
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::services::{self, NewService, ServiceInstance};
use crate::platform::windows::ports;

/// Range PHP backends are assigned from (loopback only).
const FASTCGI_PORTS: std::ops::RangeInclusive<u16> = 9000..=9999;

impl Supervisor {
    pub fn service(&self, id: &str) -> Result<ServiceInstance, Failure> {
        self.get(id)
    }

    /// `services/<id>` — the service's Devluma-managed folder.
    pub fn service_dir(&self, id: &str) -> PathBuf {
        self.paths.services.join(id)
    }

    /// The FastCGI backend for a PHP installation: created if missing, started if not
    /// running. Runs inline (blocking) as part of the caller's operation.
    pub fn ensure_php_backend(
        self: &Arc<Self>,
        installation_id: &str,
        op: &str,
    ) -> Result<ServiceInstance, Failure> {
        let inst = self
            .db
            .read(|c| installations::get(c, installation_id))
            .map_err(db_fail)?;
        if inst.package_id != "php" || inst.status != InstallStatus::Installed {
            return Err(failure(
                ErrorCode::ValidationError,
                "The selected PHP version isn't installed.",
            ));
        }
        let all = self.db.read(services::list).map_err(db_fail)?;
        let existing = all
            .iter()
            .find(|s| {
                s.package_installation_id == installation_id
                    && s.lifecycle_adapter == AdapterId::Php.as_str()
            })
            .cloned();
        let svc = match existing {
            Some(s) => s,
            None => {
                let port = FASTCGI_PORTS
                    .clone()
                    .find(|p| {
                        !all.iter().any(|s| s.port == Some(i64::from(*p))) && ports::is_free(*p)
                    })
                    .ok_or_else(|| {
                        failure(
                            ErrorCode::PortInUse,
                            "No free port is left in 9000–9999 for a PHP backend.",
                        )
                    })?;
                let version = inst
                    .package_version_id
                    .split('@')
                    .nth(1)
                    .unwrap_or("php")
                    .replace('.', "-");
                let name = format!("php-{version}");
                let svc = self
                    .db
                    .write(|c| {
                        services::create(
                            c,
                            &NewService {
                                package_installation_id: installation_id,
                                instance_name: &name,
                                service_type: "fastcgi",
                                lifecycle_adapter: AdapterId::Php.as_str(),
                                port: Some(port),
                                admin_port: None,
                                bind_address: Some("127.0.0.1"),
                                config_path: None,
                                data_path: None,
                            },
                        )
                    })
                    .map_err(db_fail)?;
                self.emit(&svc.id);
                svc
            }
        };
        if matches!(
            svc.observed_state,
            ServiceState::Running | ServiceState::Degraded
        ) {
            return Ok(svc);
        }
        if !svc.observed_state.can_start() {
            return Err(failure(
                ErrorCode::ValidationError,
                format!(
                    "The PHP backend {} is {}; try again in a moment.",
                    svc.instance_name,
                    svc.observed_state.as_str()
                ),
            ));
        }
        let _guard = self
            .claim(&svc.id)
            .map_err(|e| failure(e.code, e.message))?;
        self.do_start(op, &svc.id)?;
        self.get(&svc.id)
    }

    /// The port a PHP installation's backend uses, or would be given (for previews).
    pub fn php_backend_port(&self, installation_id: &str) -> Result<u16, Failure> {
        let all = self.db.read(services::list).map_err(db_fail)?;
        if let Some(port) = all
            .iter()
            .find(|s| {
                s.package_installation_id == installation_id
                    && s.lifecycle_adapter == AdapterId::Php.as_str()
            })
            .and_then(|s| s.port)
            .and_then(|p| u16::try_from(p).ok())
        {
            return Ok(port);
        }
        FASTCGI_PORTS
            .clone()
            .find(|p| !all.iter().any(|s| s.port == Some(i64::from(*p))) && ports::is_free(*p))
            .ok_or_else(|| {
                failure(
                    ErrorCode::PortInUse,
                    "No free port is left in 9000–9999 for a PHP backend.",
                )
            })
    }

    /// Regenerates a web server's managed config and runs its native validator.
    pub fn validate_config(&self, id: &str) -> Result<(), Failure> {
        let svc = self.get(id)?;
        let (ctx, beh) = self.context(&svc)?;
        beh.prepare(&ctx)?;
        beh.validate(&ctx)
    }

    /// Reloads a running service's configuration. Returns false if it isn't running.
    pub fn reload_if_running(&self, id: &str) -> Result<bool, Failure> {
        let svc = self.get(id)?;
        if !matches!(
            svc.observed_state,
            ServiceState::Running | ServiceState::Degraded
        ) {
            return Ok(false);
        }
        let (ctx, beh) = self.context(&svc)?;
        beh.reload(&ctx)?;
        Ok(true)
    }
}
