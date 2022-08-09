use std::{borrow::Cow, env};

use anyhow::{anyhow, Context};
use error::AppError;
use oauth2::{
    basic::BasicClient, reqwest::async_http_client, AuthUrl, AuthorizationCode, ClientId,
    ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use url::Url;

mod api;
mod error;
mod tweet;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    dotenvy::dotenv().ok();

    let (access_token, refresh_token) = authenticate().await?;
    let client = api::ApiClient::new(access_token).await?;
    let tweets = client.timeline(None).await?;
    println!("{:?}", tweets);

    // TODO: listen to request over sockets...

    // TODO: obtain timeline tweets from twitter API
    // TODO: call filters in Lua
    // TODO: return filtered tweets over the socket
    // TODO: mock frontend
    // TODO: socket protocol?

    Ok(())
}

// Autenticate to Twitter.
// TODO: cache access token / refresh tokens locally?
async fn authenticate() -> Result<(String, String), AppError> {
    let client_id =
        env::var("TWITTER_CLIENT_ID").context("Twitter OAuth2 client id is not available")?;
    let client_secret = env::var("TWITTER_CLIENT_SECRET")
        .context("Twitter OAuth2 client secret is not available")?;

    let client = create_client(client_id, client_secret)?
        .set_redirect_uri(RedirectUrl::new("https://localhost".to_owned())?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("users.read".to_owned()))
        .add_scope(Scope::new("tweet.read".to_owned()))
        .add_scope(Scope::new("offline.access".to_owned()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    println!("Browse to: {}", auth_url);
    // TODO: use a web server instead?
    println!("Paste the URL where you were redirected: ");

    let mut redirected_url = String::new();
    let stdin = std::io::stdin();
    stdin
        .read_line(&mut redirected_url)
        .context("could not read STDIN")?;
    let pairs = Url::parse(&redirected_url)?;
    let auth_code = pairs
        .query_pairs()
        .find_map(|(k, v)| match k {
            Cow::Borrowed("code") => Some(v),
            _ => None,
        })
        .context("no authorization code was returned")?
        .to_string();
    let state_returned = pairs
        .query_pairs()
        .find_map(|(k, v)| match k {
            Cow::Borrowed("state") => Some(v.to_string()),
            _ => None,
        })
        .context("no state was returned")?;
    if state.secret() != &state_returned {
        return Err(AppError::OAuth(anyhow!("invalid csrf state")));
    }

    let result = client
        .exchange_code(AuthorizationCode::new(auth_code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(async_http_client)
        .await
        .context("failed to exchange authorization code for access token")?;
    let access_token = result.access_token().secret().to_owned();
    let refresh_token = match result.refresh_token() {
        Some(x) => x.secret(),
        None => "",
    }
    .to_owned();

    Ok((access_token, refresh_token))
}

fn create_client(id: String, secret: String) -> Result<BasicClient, AppError> {
    Ok(BasicClient::new(
        ClientId::new(id),
        Some(ClientSecret::new(secret)),
        AuthUrl::new("https://twitter.com/i/oauth2/authorize".to_owned())
            .map_err(|x| AppError::OAuth(x.into()))?,
        Some(
            TokenUrl::new("https://api.twitter.com/2/oauth2/token".to_owned())
                .map_err(|x| AppError::OAuth(x.into()))?,
        ),
    ))
}
