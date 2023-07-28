use std::sync::Arc;

use poem::{listener::TcpListener, web::Data, EndpointExt, Route, Server};
use poem_openapi::{param::Query, payload::Json, OpenApi, OpenApiService};
use poem_openapi_derive::{ApiResponse, Object};

use pokemon_api::Pokedex;
use reqwest::Url;
use serde::Serialize;
use thiserror::Error;
use tracing::{error, Level};

const BASE_POKEMONAPI_ADDRESS: &'static str = "https://pokeapi.co/api/v2/";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .init();
    let pokedex = Arc::new(Pokedex::new(BASE_POKEMONAPI_ADDRESS)?);
    let api_service = OpenApiService::new(Api, "Demo", "1.0").server("http://localhost:3001/api");
    let ui = api_service.swagger_ui();
    Server::new(TcpListener::bind(":::3001"))
        .run(
            Route::new()
                .nest("/api", api_service.data(pokedex))
                .nest("/", ui),
        )
        .await?;
    Ok(())
}

struct Api;

#[derive(ApiResponse)]
enum PokemonListResponse {
    #[oai(status = 200)]
    Ok(Json<Vec<Pokemon>>),
    #[oai(status = 500)]
    InternalServerError,
}

#[derive(Serialize, Object)]
struct Pokemon {
    pub id: u32,
    pub name: String,
}

#[OpenApi]
impl Api {
    #[tracing::instrument(level=tracing::Level::INFO,skip(self, pokedex,))]
    #[oai(path = "/pokemon", method = "get")]
    async fn pokemon(
        &self,
        Data(pokedex): Data<&Arc<pokemon_api::Pokedex>>,
        Query(limit): Query<Option<u32>>,
        Query(offset): Query<Option<u32>>,
    ) -> PokemonListResponse {
        match pokedex
            .get_all_pokemon(limit.unwrap_or(20), offset.unwrap_or(0))
            .await
        {
            Ok(r) => {
                let mut data = r.results;
                #[derive(Error, Debug, Copy, Clone, Eq, PartialEq)]
                #[error("Invalid url in pokemon response: expecting url with numeric id in last segment")]
                struct InvalidUrlInResponse;

                let result: Result<Vec<Pokemon>, _> = data
                    .drain(..)
                    .map(|i| -> Result<Pokemon, InvalidUrlInResponse> {
                        let url: Url = i.url.parse().map_err(|_| InvalidUrlInResponse)?;
                        let name = i.name;
                        let segments = url.path_segments().ok_or(InvalidUrlInResponse)?;
                        let last_segment = segments.last().ok_or(InvalidUrlInResponse)?;
                        let id = last_segment.parse().map_err(|_| InvalidUrlInResponse)?;

                        Ok(Pokemon { id, name })
                    })
                    .collect();
                match result {
                    Ok(r) => PokemonListResponse::Ok(Json(r)),
                    Err(e) => {
                        error!(err = %e);
                        PokemonListResponse::InternalServerError
                    }
                }
            }
            Err(_) => PokemonListResponse::InternalServerError,
        }
    }
}

mod pokemon_api {
    use reqwest::Url;
    use serde::{Deserialize, Serialize};
    use thiserror::Error;
    use tracing::instrument;

    #[derive(Deserialize, Serialize)]
    pub struct Pokemon {
        pub url: String,
        pub name: String,
    }

    #[derive(Deserialize)]
    pub struct PokemonList {
        pub count: u32,
        pub next: Option<String>,
        pub previous: Option<String>,
        pub results: Vec<Pokemon>,
    }

    #[derive(Error, Debug)]
    pub(crate) enum PokedexError {
        #[error("Error during http request: {0}")]
        HttpRequestError(#[from] reqwest::Error),
        #[error("Invalid base url")]
        InvalidBaseUrl,
    }
    pub(crate) struct Pokedex {
        http_client: reqwest::Client,
        base: Url,
    }

    impl Pokedex {
        #[instrument(skip(self), err)]
        pub async fn get_all_pokemon(
            &self,
            limit: u32,
            offset: u32,
        ) -> Result<PokemonList, PokedexError> {
            let url = self
                .base
                .join("pokemon")
                .expect("could not join with base url");
            Ok(self
                .http_client
                .get(url)
                .query(&[("limit", limit), ("offset", offset)])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?)
        }

        pub fn new(base: &str) -> Result<Self, PokedexError> {
            let client = reqwest::Client::builder().build().unwrap();
            let base = Url::try_from(base).map_err(|_| PokedexError::InvalidBaseUrl)?;
            if base.cannot_be_a_base() {
                return Err(PokedexError::InvalidBaseUrl);
            }
            if base.scheme() != "http" && base.scheme() != "https" {
                return Err(PokedexError::InvalidBaseUrl);
            }
            Ok(Self {
                base,
                http_client: client,
            })
        }
    }
}
