//! Service instances. `observed_state` is a cache of the last probe, not proof a process is
//! running; startup reconciliation resets it to `unknown` until re-probed (schema.md §1).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, new_id, now};

pub use crate::domain::service::ServiceState;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInstance {
    pub id: String,
    pub package_installation_id: String,
    pub instance_name: String,
    pub service_type: String,
    pub lifecycle_adapter: String,
    pub desired_state: String,
    pub observed_state: ServiceState,
    pub port: Option<i64>,
    pub bind_address: Option<String>,
    pub config_path: Option<String>,
    pub data_path: Option<String>,
    pub pid: Option<i64>,
    /// Process creation time (FILETIME as a decimal string), part of the ownership proof.
    pub process_started_at: Option<String>,
    pub health_status: String,
    pub last_exit_code: Option<i64>,
    pub last_error_code: Option<String>,
    /// Job object owning the current run (unique per start).
    pub job_name: Option<String>,
    /// Second port for the tool's own UI (e.g. Mailpit's web interface), if it has one.
    pub admin_port: Option<i64>,
    pub updated_at: String,
}

const COLUMNS: &str = "id, package_installation_id, instance_name, service_type, lifecycle_adapter, \
     desired_state, observed_state, port, bind_address, config_path, data_path, pid, process_started_at, updated_at, \
     health_status, last_exit_code, last_error_code, job_name, admin_port";

fn from_row(r: &Row<'_>) -> rusqlite::Result<ServiceInstance> {
    let state: String = r.get(6)?;
    Ok(ServiceInstance {
        id: r.get(0)?,
        package_installation_id: r.get(1)?,
        instance_name: r.get(2)?,
        service_type: r.get(3)?,
        lifecycle_adapter: r.get(4)?,
        desired_state: r.get(5)?,
        observed_state: ServiceState::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, state.into())
        })?,
        port: r.get(7)?,
        bind_address: r.get(8)?,
        config_path: r.get(9)?,
        data_path: r.get(10)?,
        pid: r.get(11)?,
        process_started_at: r.get(12)?,
        updated_at: r.get(13)?,
        health_status: r.get(14)?,
        last_exit_code: r.get(15)?,
        last_error_code: r.get(16)?,
        job_name: r.get(17)?,
        admin_port: r.get(18)?,
    })
}

#[derive(Debug, Clone, Default)]
pub struct NewService<'a> {
    pub package_installation_id: &'a str,
    pub instance_name: &'a str,
    pub service_type: &'a str,
    pub lifecycle_adapter: &'a str,
    pub port: Option<u16>,
    pub admin_port: Option<u16>,
    pub bind_address: Option<&'a str>,
    pub config_path: Option<&'a str>,
    pub data_path: Option<&'a str>,
}

pub fn create(conn: &Connection, s: &NewService<'_>) -> DbResult<ServiceInstance> {
    let id = new_id();
    conn.execute(
        "INSERT INTO service_instances (id, package_installation_id, instance_name, service_type,
              lifecycle_adapter, desired_state, observed_state, port, bind_address, config_path,
              data_path, updated_at, admin_port)
         VALUES (?1, ?2, ?3, ?4, ?5, 'stopped', 'stopped', ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            s.package_installation_id,
            s.instance_name,
            s.service_type,
            s.lifecycle_adapter,
            s.port,
            s.bind_address,
            s.config_path,
            s.data_path,
            now(),
            s.admin_port
        ],
    )?;
    get(conn, &id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<ServiceInstance> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM service_instances WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "service" })
}

pub fn list(conn: &Connection) -> DbResult<Vec<ServiceInstance>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM service_instances ORDER BY service_type, instance_name"
    ))?;
    Ok(stmt.query_map([], from_row)?.collect::<Result<_, _>>()?)
}

pub fn set_observed_state(conn: &Connection, id: &str, state: ServiceState) -> DbResult<()> {
    let n = conn.execute(
        "UPDATE service_instances SET observed_state = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, state.as_str(), now()],
    )?;
    if n == 0 {
        return Err(DbError::NotFound { entity: "service" });
    }
    Ok(())
}

/// Moves `id` to `to` if the state machine allows it from the current state.
pub fn transition(conn: &Connection, id: &str, to: ServiceState) -> DbResult<ServiceInstance> {
    let current = get(conn, id)?;
    if current.observed_state == to {
        return Ok(current);
    }
    if !current.observed_state.can_transition_to(to) {
        return Err(DbError::Invariant(format!(
            "Service can't go from {} to {}.",
            current.observed_state.as_str(),
            to.as_str()
        )));
    }
    set_observed_state(conn, id, to)?;
    get(conn, id)
}

pub fn set_desired(conn: &Connection, id: &str, desired: &str) -> DbResult<()> {
    conn.execute(
        "UPDATE service_instances SET desired_state = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, desired, now()],
    )?;
    Ok(())
}

/// Records (or clears) the owned process: PID plus creation time.
pub fn set_process(
    conn: &Connection,
    id: &str,
    pid: Option<u32>,
    created: Option<&str>,
    job_name: Option<&str>,
) -> DbResult<()> {
    conn.execute(
        "UPDATE service_instances SET pid = ?2, process_started_at = ?3, job_name = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, pid, created, job_name, now()],
    )?;
    Ok(())
}

pub fn set_outcome(
    conn: &Connection,
    id: &str,
    exit_code: Option<i64>,
    error_code: Option<&str>,
) -> DbResult<()> {
    conn.execute(
        "UPDATE service_instances SET last_exit_code = ?2, last_error_code = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, exit_code, error_code, now()],
    )?;
    Ok(())
}

pub fn set_health(conn: &Connection, id: &str, health: &str) -> DbResult<()> {
    conn.execute(
        "UPDATE service_instances SET health_status = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, health, now()],
    )?;
    Ok(())
}

pub fn set_paths(
    conn: &Connection,
    id: &str,
    config_path: Option<&str>,
    data_path: Option<&str>,
) -> DbResult<()> {
    conn.execute(
        "UPDATE service_instances SET config_path = ?2, data_path = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, config_path, data_path, now()],
    )?;
    Ok(())
}

/// Deletes the instance record (and its dependency rows). Files are handled by the caller.
pub fn delete(conn: &Connection, id: &str) -> DbResult<()> {
    if conn.execute("DELETE FROM service_instances WHERE id = ?1", [id])? == 0 {
        return Err(DbError::NotFound { entity: "service" });
    }
    Ok(())
}

/// Startup reconciliation: nothing has been probed yet, so no cached state is trustworthy.
/// Returns the number of services changed.
pub fn mark_all_unknown(conn: &Connection) -> DbResult<usize> {
    Ok(conn.execute(
        "UPDATE service_instances SET observed_state = 'unknown', updated_at = ?1
         WHERE observed_state NOT IN ('unknown', 'not_installed')",
        [now()],
    )?)
}

pub fn set_ports(conn: &Connection, id: &str, port: u16, admin_port: Option<u16>) -> DbResult<()> {
    let n = conn.execute(
        "UPDATE service_instances SET port = ?2, admin_port = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, port, admin_port, now()],
    )?;
    if n == 0 {
        return Err(DbError::NotFound { entity: "service" });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::installations::fixtures::installed;
    use crate::persistence::is_constraint_violation;
    use crate::persistence::test_support::db;

    #[test]
    fn port_range_is_enforced() {
        let db = db();
        let err = db
            .write(|c| {
                let i = installed(c, "nginx", "1.27.0", "n")?;
                create(
                    c,
                    &NewService {
                        package_installation_id: &i.id,
                        instance_name: "default",
                        service_type: "web_server",
                        lifecycle_adapter: "nginx",
                        port: Some(80),
                        ..Default::default()
                    },
                )?;
                Ok(c.execute("UPDATE service_instances SET port = 70000", [])?)
            })
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlite(ref e) if is_constraint_violation(e)));
    }

    #[test]
    fn observed_state_round_trips_and_resets() {
        let db = db();
        let id = db
            .write(|c| {
                let i = installed(c, "mariadb", "11.4.0", "m")?;
                let s = create(
                    c,
                    &NewService {
                        package_installation_id: &i.id,
                        instance_name: "default",
                        service_type: "database",
                        lifecycle_adapter: "mariadb",
                        port: Some(3306),
                        ..Default::default()
                    },
                )?;
                set_observed_state(c, &s.id, ServiceState::Running)?;
                Ok(s.id)
            })
            .unwrap();
        assert_eq!(
            db.read(|c| get(c, &id)).unwrap().observed_state,
            ServiceState::Running
        );
        assert_eq!(db.write(|c| mark_all_unknown(c)).unwrap(), 1);
        assert_eq!(
            db.read(|c| get(c, &id)).unwrap().observed_state,
            ServiceState::Unknown
        );
    }
}
