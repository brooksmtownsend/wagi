use crate::http_util::*;
use crate::runtime::*;

use hyper::{Body, Request, Response};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;

mod http_util;
pub mod runtime;
pub mod version;

/// A router is responsible for taking an inbound request and sending it to the appropriate handler.
pub struct Router {
    pub module_config: ModuleConfig,
    pub cache_config_path: String,
}

impl Router {
    /// Route the request to the correct handler
    ///
    /// Some routes are built in (like healthz), while others are dynamically
    /// dispatched.
    pub async fn route(
        &self,
        req: Request<Body>,
        client_addr: SocketAddr,
    ) -> Result<Response<Body>, hyper::Error> {
        // TODO: Improve the loading. See issue #3
        //
        // Additionally, we could implement an LRU to cache WASM modules. This would
        // greatly reduce the amount of load time per request. But this would come with two
        // drawbacks: (a) it would be different than CGI, and (b) it would involve a cache
        // clear during debugging, which could be a bit annoying.

        let uri_path = req.uri().path();
        match uri_path {
            "/healthz" => Ok(Response::new(Body::from("OK"))),
            _ => match self.module_config.handler_for_path(uri_path) {
                Ok(h) => Ok(h
                    .module
                    .execute(
                        h.entrypoint.as_str(),
                        req,
                        client_addr,
                        self.cache_config_path.clone(),
                    )
                    .await),
                Err(e) => {
                    eprintln!("error: {}", e);
                    Ok(not_found())
                }
            },
        }
    }
}

/// Load the configuration TOML
pub fn load_modules_toml(
    filename: &str,
    cache_config_path: String,
) -> Result<ModuleConfig, anyhow::Error> {
    if !Path::new(filename).is_file() {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            filename
        ));
    }

    let data = std::fs::read_to_string(filename)?;
    let mut modules: ModuleConfig = toml::from_str(data.as_str())?;

    modules.build_registry(cache_config_path)?;

    Ok(modules)
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Deserialize)]
pub struct ModuleConfig {
    /// this line de-serializes [[module]] as modules
    #[serde(rename = "module")]
    pub modules: Vec<crate::runtime::Module>,

    /// Cache of routes.
    ///
    /// This is built by calling `build_registry`.
    #[serde(skip)]
    route_cache: Option<Vec<Handler>>,
}

impl ModuleConfig {
    /// Construct a registry of all routes.
    fn build_registry(&mut self, cache_config_path: String) -> anyhow::Result<()> {
        let mut routes = vec![];

        let mut failed_modules: Vec<String> = Vec::new();

        self.modules.iter().for_each(|m| {
            match m.load_routes(cache_config_path.clone()) {
                Err(e) => {
                    // FIXME: I think we could do something better here.
                    failed_modules.push(e.to_string())
                }
                Ok(subroutes) => subroutes
                    .iter()
                    .for_each(|entry| routes.push(Handler::new(entry, m))),
            }
        });

        if !failed_modules.is_empty() {
            let msg = failed_modules.join(", ");
            return Err(anyhow::anyhow!("Not all routes could be built: {}", msg));
        }

        self.route_cache = Some(routes);
        Ok(())
    }

    /// Given a URI fragment, find the handler that can execute this.
    fn handler_for_path(&self, uri_fragment: &str) -> Result<Handler, anyhow::Error> {
        if let Some(routes) = self.route_cache.as_ref() {
            for r in routes {
                // The important detail here is that strip_suffix returns None if the suffix
                // does not exist. So ONLY paths that end with /... are substring-matched.
                if r.path
                    .strip_suffix("/...")
                    .map(|i| {
                        println!("Comparing {} to {}", uri_fragment.clone(), r.path.as_str());
                        uri_fragment.starts_with(i)
                    })
                    .unwrap_or_else(|| r.path == uri_fragment)
                {
                    return Ok(r.clone());
                }
            }
        }

        Err(anyhow::anyhow!("No handler for {}", uri_fragment))
    }
}
