use common::{api, channel, frontend, service};
use std::{fmt, net, str::FromStr, sync, thread};
#[cfg(target_os = "windows")]
use windows as w;

#[cfg(all(any(feature = "dvc", feature = "svc"), feature = "service-input"))]
mod client;
mod config;
#[cfg(any(feature = "dvc", feature = "svc"))]
mod control;
#[cfg(any(feature = "dvc", feature = "svc", feature = "sangfor"))]
mod vc;

enum Error {
    Binding(String),
    Config(config::Error),
    #[cfg(any(feature = "dvc", feature = "svc"))]
    Control(control::Error),
    #[cfg(any(feature = "dvc", feature = "svc"))]
    Vc(vc::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Self::Binding(e) => write!(f, "binding error: {e}"),
            Self::Config(e) => write!(f, "configuration error: {e}"),
            #[cfg(any(feature = "dvc", feature = "svc"))]
            Self::Control(e) => write!(f, "control error: {e}"),
            #[cfg(any(feature = "dvc", feature = "svc"))]
            Self::Vc(e) => write!(f, "virtual channel error: {e}"),
        }
    }
}

impl From<config::Error> for Error {
    fn from(e: config::Error) -> Self {
        Self::Config(e)
    }
}

#[cfg(any(feature = "dvc", feature = "svc"))]
impl From<control::Error> for Error {
    fn from(e: control::Error) -> Self {
        Self::Control(e)
    }
}

#[cfg(any(feature = "dvc", feature = "svc"))]
impl From<vc::Error> for Error {
    fn from(e: vc::Error) -> Self {
        Self::Vc(e)
    }
}

static CONFIG: sync::OnceLock<config::Config> = sync::OnceLock::new();

#[cfg(any(feature = "dvc", feature = "svc"))]
static CONTROL: sync::LazyLock<control::Control> = sync::LazyLock::new(control::Control::new);

fn bootstrap() -> Result<&'static config::Config, Error> {
    if let Some(config) = CONFIG.get() {
        return Ok(config);
    }

    let config = match config::Config::read()? {
        None => {
            let config = config::Config::default();
            config.save()?;
            config
        }
        Some(config) => config,
    };

    #[cfg(target_os = "windows")]
    let _ = unsafe { w::Win32::System::Console::AllocConsole() };

    common::init_logs(config.log_level(), config.log_file());

    common::log_package_infos!();

    common::debug!("bootstrapping frontend");

    Ok(CONFIG.get_or_init(|| config))
}

#[allow(clippy::missing_panics_doc)]
fn start_res(
    config: &config::Config,
    frontend_channel: channel::Channel,
    backend_to_frontend: crossbeam_channel::Receiver<api::Message>,
) -> Result<(), Error> {
    let servers = config.services.iter().filter(|s| s.enabled).try_fold(
        vec![],
        |mut servers, service_conf| match service::lookup(service_conf.name.as_str()) {
            None => {
                common::error!("unknown service {}", service_conf.name);
                Ok(servers)
            }
            Some(service) => {
                if service.internal() {
                    common::error!("service {} is an internal service", service_conf.name);
                    Ok(servers)
                } else {
                    match service.frontend().and_then(frontend::Frontend::tcp) {
                        None => Ok::<_, Error>(servers),
                        Some(frontend::FrontendTcp { default_port, .. }) => {
                            let ip = net::IpAddr::from_str(
                                &service_conf.ip.clone().unwrap_or(config.ip.clone()),
                            )
                            .map_err(|e| Error::Binding(e.to_string()))?;
                            let port = service_conf.port.unwrap_or(*default_port);
                            let sockaddr = net::SocketAddr::new(ip, port);

                            let server = frontend::FrontendTcpServer::bind(service, sockaddr, None)
                                .map_err(|e| Error::Binding(e.to_string()))?;

                            servers.push(server);

                            Ok(servers)
                        }
                    }
                }
            }
        },
    )?;

    #[cfg(not(feature = "service-forward"))]
    {
        if config.forward.as_ref().is_some_and(|v| !v.is_empty()) {
            common::error!(
                "ignoring port forwarding entries has support of port forwarding is not enabled"
            );
        }
    }

    #[cfg(feature = "service-forward")]
    let servers = match config.forward.as_ref() {
        None => servers,
        Some(forwards) => forwards.iter().filter(|s| s.enabled).try_fold(
            servers,
            |mut servers, forward_conf| {
                let ip = net::IpAddr::from_str(&forward_conf.ip)
                    .map_err(|e| Error::Binding(e.to_string()))?;
                let port = forward_conf.port;
                let sockaddr = net::SocketAddr::new(ip, port);

                let server = frontend::FrontendTcpServer::bind(
                    &common::forward::SERVICE,
                    sockaddr,
                    Some(forward_conf.destination.clone()),
                )
                .map_err(|e| Error::Binding(e.to_string()))?;

                servers.push(server);

                Ok::<_, Error>(servers)
            },
        )?,
    };

    thread::Builder::new()
        .name("frontend".into())
        .spawn(move || {
            thread::scope(|scope| {
                #[cfg(any(feature = "dvc", feature = "svc"))]
                CONTROL.start(scope);

                for server in &servers {
                    thread::Builder::new()
                        .name(server.service().name().to_string())
                        .spawn_scoped(scope, || {
                            if let Err(e) = server.start(&frontend_channel) {
                                common::error!("{} error: {e}", server.service().name());
                            } else {
                                common::debug!("{} terminated", server.service().name());
                            }
                        })
                        .unwrap();
                }

                if let Err(e) = frontend_channel.run(service::Kind::Frontend, &backend_to_frontend)
                {
                    common::error!("frontend channel stopped: {e}");
                } else {
                    common::debug!("frontend channel stopped");
                }
            });
        })
        .unwrap();

    Ok(())
}

#[cfg(any(feature = "dvc", feature = "svc"))]
fn init() {
    match bootstrap() {
        Err(e) => {
            eprintln!("{e}");
        }
        Ok(config) => {
            common::debug!("init frontend");

            if let Some(connector) = CONTROL.take_frontend_connector() {
                let services = channel::Channel::new(connector.send);

                if let Err(e) = start_res(config, services, connector.recv) {
                    common::error!("frontend init: {e}");
                }
            }
        }
    }
}

pub fn start(
    frontend_channel: channel::Channel,
    backend_to_frontend: crossbeam_channel::Receiver<api::Message>,
) -> Result<(), String> {
    let config = bootstrap().map_err(|e| e.to_string())?;
    start_res(config, frontend_channel, backend_to_frontend).map_err(|e| e.to_string())
}
