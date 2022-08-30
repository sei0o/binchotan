use crate::{
    api::ApiClient, error::AppError, filter::Filter, methods::HttpMethod, tweet::Tweet, VERSION,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use tracing::{info, warn};

pub const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub method: Method,
    #[serde(default)]
    pub params: RequestParams,
    pub id: String,
}

impl Request {
    pub fn validate(&self) -> Result<(), AppError> {
        match self.jsonrpc.as_str() {
            JSONRPC_VERSION => Ok(()),
            v => Err(AppError::RpcVersion(v.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub enum Method {
    #[serde(rename = "v0.plain")]
    Plain,
    #[serde(rename = "v0.home_timeline")]
    HomeTimeline,
    #[serde(rename = "v0.status")]
    Status,
    #[serde(rename = "v0.account.list")]
    AccountList,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RequestParams {
    Plain {
        user_id: String,
        http_method: HttpMethod,
        endpoint: String,
        api_params: HashMap<String, serde_json::Value>,
    },
    MapWithId {
        user_id: String,
        api_params: HashMap<String, serde_json::Value>,
    },
    Map(HashMap<String, serde_json::Value>),
}

impl Default for RequestParams {
    fn default() -> Self {
        RequestParams::Map(HashMap::new())
    }
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub content: ResponseContent,
    pub id: String,
}

#[derive(Debug, Serialize)]
pub enum ResponseContent {
    #[serde(rename = "result")]
    Plain {
        meta: ResponsePlainMeta,
        body: serde_json::Value,
    },
    #[serde(rename = "result")]
    HomeTimeline {
        meta: ResponsePlainMeta,
        body: Vec<Tweet>,
    },
    #[serde(rename = "result")]
    Status { version: String },
    #[serde(rename = "result")]
    AccountList { user_ids: Vec<String> },
    #[serde(rename = "error")]
    Error(ResponseError),
}

#[derive(Debug, Serialize)]
pub struct ResponsePlainMeta {
    pub api_calls_remaining: usize,
    pub api_calls_reset: usize, // in epoch sec
}

#[derive(Debug, Serialize)]
pub struct ResponseError {
    pub code: isize,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl From<AppError> for ResponseError {
    fn from(err: AppError) -> Self {
        let code = match err {
            AppError::Io(_) => -32000,
            AppError::ApiResponseParse(_) => -32000,
            AppError::ApiResponseNotFound(_, _) => -32000,
            AppError::ApiResponseSerialize(_) => -32000,
            AppError::ApiRequest(_) => -32000,
            AppError::ApiResponseStatus(_, _) => -32001,
            AppError::OAuth(_) => -32000,
            AppError::OAuthUrlParse(_) => -32000,
            AppError::SocketPayloadParse(_) => -32700,
            AppError::RpcVersion(_) => -32600,
            AppError::RpcParamsParse(_) => -32700,
            AppError::RpcParamsMismatch(_) => -32602,
            AppError::RpcTooLarge => -32603,
            AppError::Lua(_) => -32002,
            AppError::Other(_) => -32099,
            AppError::ApiExpiredToken => -32000,
            // errors which should be thrown during initialization
            _ => unreachable!(),
        };

        ResponseError {
            code,
            message: err.to_string(),
            data: None,
        }
    }
}

pub struct Handler {
    pub clients: HashMap<String, ApiClient>,
    pub filter_path: PathBuf,
    pub scopes: HashSet<String>,
}

impl Handler {
    pub async fn handle(&self, req: Request) -> Response {
        let id = req.id.clone();
        match self.handle_inner(req).await {
            Ok(resp) => resp,
            Err(err) => {
                warn!("something bad happened: {:?}", err);
                let resp_err: ResponseError = err.into();
                Response {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    content: ResponseContent::Error(resp_err),
                    id,
                }
            }
        }
    }

    async fn handle_inner(&self, req: Request) -> Result<Response, AppError> {
        info!("received a request: {:?}", req);
        req.validate()?;

        match req.method {
            Method::Plain => self.handle_plain(req).await,
            Method::HomeTimeline => self.handle_timeline(req).await,
            Method::Status => self.handle_status(req).await,
            Method::AccountList => self.handle_account_list(req).await,
        }
    }

    async fn handle_plain(&self, req: Request) -> Result<Response, AppError> {
        match req.params {
            RequestParams::Plain {
                user_id,
                http_method,
                endpoint,
                api_params,
            } => {
                let client = self
                    .clients
                    .get(&user_id)
                    .ok_or(AppError::RpcUnknownAccount(user_id))?;
                let resp = client.call(&http_method, &endpoint, &api_params).await?;
                info!("got response for plain request with id {}", req.id);

                let content = ResponseContent::Plain {
                    meta: ResponsePlainMeta {
                        // TODO:
                        api_calls_remaining: 0,
                        api_calls_reset: 0,
                    },
                    body: resp,
                };
                Ok(Response {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    content,
                    id: req.id,
                })
            }
            _ => Err(AppError::RpcParamsMismatch(req)),
        }
    }
    async fn handle_timeline(&self, req: Request) -> Result<Response, AppError> {
        let (user_id, mut params) = match req.params {
            RequestParams::MapWithId {
                user_id,
                api_params,
            } => (user_id, api_params),
            _ => return Err(AppError::RpcParamsMismatch(req)),
        };
        let client = self
            .clients
            .get(&user_id)
            .ok_or(AppError::RpcUnknownAccount(user_id))?;
        let tweets = client.timeline(&mut params).await?;
        info!(
            "successfully retrieved {} tweets (reverse_chronological). here's one of them: {:?}",
            tweets.len(),
            tweets[0]
        );

        let filters = Filter::load(self.filter_path.as_ref(), &self.scopes)?;

        let mut filtered_tweets = vec![];
        'outer: for tweet in tweets {
            let mut result = tweet;
            for filter in &filters {
                match filter.run(&result)? {
                    Some(t) => result = t,
                    None => continue 'outer,
                }
            }
            filtered_tweets.push(result);
        }

        let content = ResponseContent::HomeTimeline {
            meta: ResponsePlainMeta {
                // TODO:
                api_calls_remaining: 0,
                api_calls_reset: 0,
            },
            body: filtered_tweets,
        };
        Ok(Response {
            jsonrpc: JSONRPC_VERSION.to_string(),
            content,
            id: req.id,
        })
    }
    async fn handle_status(&self, req: Request) -> Result<Response, AppError> {
        let req_ = req.clone();
        match req.params {
            RequestParams::Map(prms) => {
                if !prms.is_empty() {
                    return Err(AppError::RpcParamsMismatch(req_));
                }

                let content = ResponseContent::Status {
                    version: VERSION.to_string(),
                };

                Ok(Response {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    content,
                    id: req.id,
                })
            }
            _ => Err(AppError::RpcParamsMismatch(req)),
        }
    }
    async fn handle_account_list(&self, req: Request) -> Result<Response, AppError> {
        let req_ = req.clone();
        match req.params {
            RequestParams::Map(prms) => {
                if !prms.is_empty() {
                    return Err(AppError::RpcParamsMismatch(req_));
                }

                let content = ResponseContent::AccountList {
                    user_ids: self.clients.keys().cloned().collect(),
                };

                Ok(Response {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    content,
                    id: req.id,
                })
            }
            _ => Err(AppError::RpcParamsMismatch(req)),
        }
    }
}
