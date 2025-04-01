mod message;

use core::fmt;
use std::{process::{self, Command}, error::Error};
use std::str;

use config_file::FromConfigFile;
use log::{info, error};
use message::{AuthMessage, CredentialMessage};
use reqwest::header::{CONTENT_TYPE, ACCEPT};
use tokio_tungstenite::tungstenite::{Message, connect};
use url::Url;
use serde::{Serialize, Deserialize};

use crate::message::{OutboundMessage, InboundMessage};


#[derive(Serialize, Deserialize)]
struct MacronConfig {
    server: ServerConfig,
    functions: Vec<MacronFunction>

}

#[derive(Serialize, Deserialize)]
struct ServerConfig {
    url: String,
    dev: bool,
    email: String,
    password: String,
    //auth_key: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MacronFunction {
    id: u8,
    name: String,
    description: String,
    #[serde(skip_serializing)]
    command: String,
}

#[derive(Debug)]
struct MacronError {
    body: String
}

impl Error for MacronError {}

impl std::fmt::Display for MacronError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MacronError: {}", self.body)
    }
}

fn exec_function(id: usize, config: &MacronConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    match config.functions.get(id) {
        Some(func) => {

            let mut cmd = Command::new(&func.command);
            //cmd.arg("&");
            //cmd.output()?;
            cmd.spawn()?;
        }
        None => {
            return Err(Box::new(MacronError{ body: "Function not found".to_string() }))
        }
    }
    Ok(())

}

async fn login(url: String, msg: &CredentialMessage, dev: bool) -> Result<String, Box<dyn Error + Send + Sync>> {
    //TODO: Implement handling for incorrect password
    let client = reqwest::Client::new();
    let request_url: String;
    if dev {
        request_url = String::from("http://") + &url + "/v1/login";
    } else {
        request_url = String::from("https://") + &url + "/v1/login";
    }
    //let request_url = &(String::from("http://") + &url + "/v1/login");
    info!("Sending login request");
    let response = client
        .post(request_url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .json(msg)
        .send()
        .await?
        .bytes()
    .await?;

    let response_str = str::from_utf8(&response).unwrap_or("Error parsing response.");
    info!("Login response: {}", response_str);

    let auth_response: AuthMessage = serde_json::from_slice(&response)?;

    return Ok(auth_response.session_token)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>>{
    env_logger::init();
    info!("Starting macron daemon...");

    dotenv::dotenv().ok();

    //let server_url = std::env::var("SERVER_URL").expect("Server url not in environment.");
    //let auth_key = std::env::var("AUTH_KEY").expect("Auth key not in environtment.");
    let config_dir = match std::env::var("MACRON_CONFIG") {
        Ok(dir) => {
            dir
        }
        Err(_) => {
            let home = std::env::var("HOME").expect("No config file and cannot find HOME dir");
            home + "/.config/macron/config.toml"
        }
    };

    info!("Config file dir: {}", config_dir);

    let config = MacronConfig::from_config_file(config_dir).expect("Config file not found.");

    info!("Config functions: {}", config.functions.len());
    
    let creds = &CredentialMessage { email: config.server.email.clone(), password: config.server.password.clone() };
    
    let session_token = login(config.server.url.clone(), creds, config.server.dev).await?;
    let query = "session_token=".to_owned() + &session_token;

    let url_string: String;
    if config.server.dev {
        url_string = String::from("ws://") + &config.server.url + "/v1/ws/receiver";
    } else {
        url_string = String::from("wss://") + &config.server.url + "/v1/ws/receiver";
    };
    //let url_string = &(String::from("ws://") + &config.server.url + "/v1/ws/receiver");

    let mut url = Url::parse(&url_string).unwrap(); 

    url.set_query(Some(query.as_str()));
    info!("Connecting to websocket endpoint at url {}", url);
    let (mut socket, _) = connect(url)?;

    let auth_msg = OutboundMessage {
        message_type: "auth".to_string(),
        client_id: None,
        //password: Some(config.server.password.clone()),
        password: None,
        receiver_name: "rust".to_string(),
        functions: None,
    };

    info!("Sending auth message");
    let auth_json = serde_json::to_string(&auth_msg)?;
    socket.send(Message::Text(auth_json))?;

    info!("Receiving auth confrimation");
    let auth_response_json = socket.read()?;
    info!("Server response: {}", auth_response_json);
    let auth_response: InboundMessage = serde_json::from_str(&auth_response_json.to_string())?;

    if auth_response.message_type == "auth_success" {
        info!("Auth Success.");
    } else {
        error!("Cannot confirm authentication");
        process::exit(2);
    }

    info!("Starting loop...");
    loop {
        let msg_json = socket.read()?;
        info!("Message: {}", msg_json.to_string());

        let json: InboundMessage = serde_json::from_str(&msg_json.to_string())?;

        match json.message_type.as_str() {
            "functions" => {
                info!("Sending functions...");
                let response = OutboundMessage {
                    message_type: "functions".to_string(),
                    client_id: json.client_id,
                    password: Some(config.server.password.clone()),
                    //auth_key: config.server.auth_key.clone(),
                    receiver_name: "rust".to_string(),
                    functions: Some(config.functions.clone()),
                };

                let json_response = serde_json::to_string(&response)?;
                info!("Function Response: {}", json_response);
                socket.send(Message::Text(json_response))?;
            },
            "exec" => {
                info!("Executing Function...");
                let index = json.id.unwrap_or(usize::MAX);
                exec_function(index, &config)?;
                
            }
            _ => {}
        };
    }
}
