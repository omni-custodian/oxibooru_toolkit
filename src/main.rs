use errors::SzurubooruClientError;
use models::MergePost;
use post_utils::get_files;
use serde::Deserialize;
use std::error::Error as ErrError;
use std::io::{Error, ErrorKind};
use std::{env, fs, io};
use std::path::{Path, PathBuf};
use szurubooru_client::*;
use tokio::time::{sleep, Duration};
use indicatif::{ProgressBar, ProgressStyle};

mod post_utils;

#[tokio::main]
async fn main() -> Result<(), Box<dyn ErrError>> {
    let config =  load_or_create_config()?;

    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: <operation> <element> <path> [options]");
        return Ok(()); // Return Ok(()) to match the function signature
    }

    let operation = &args[1];
    let element = &args[2];
    let path = &args[3];
    let option = args.get(4);

    let client = SzurubooruClient::new_with_token(
        config.server.url.as_str(), 
        config.auth.username.as_str(), 
        config.auth.token.as_str(), 
        true,
    )?;

    match operation.as_str() {
        "set" if element == "tag_category" => {
            set_tags_to_category(&client, path, option.unwrap()).await;
            Ok(())
        }
        "list" if element == "tag_category" => {
            list_tags_of_category(&client, path, option.unwrap()).await;
            Ok(())
        }
        "upload" if element == "post" => {
            match upload_posts(&client, path, config).await {
                Ok(_) => println!("Finished uploading posts."),
                Err(e) => eprintln!("Error uploading posts: {}", e),
            }
            Ok(())
        }
        "upload" if element == "pool" => {
            upload_pool(&client, path).await;
            Ok(())
        }
        "merge" if element == "post" => {
            match merge_posts(&client, path, config).await {
                Ok(_) => println!("Finished merging posts."),
                Err(e) => eprintln!("Error merging posts: {}", e),
            }
            Ok(())
        }
        _ => {
            eprintln!("Invalid operation or element");
            Ok(())
        }
    }
}

async fn set_tags_to_category(client: &SzurubooruClient, path: &str, option: &str) {
    let path_obj = Path::new(path);
    if path_obj.is_dir() {
        eprintln!("Error: Expected a file, but a directory was provided for tag operation");
        return;
    }
}

async fn list_tags_of_category(client: &SzurubooruClient, path: &str, option: &str) {
    let path_obj = Path::new(path);
    if path_obj.is_dir() {
        eprintln!("Error: Expected a file, but a directory was provided for tag operation");
        return;
    }
    // Your logic to list tags of a category into a file
    println!("Listing tags of category at path: {}", todo!());
}

async fn upload_posts(client: &SzurubooruClient, path: &str, config: Config) -> SzurubooruResult<Vec<u32>> {
    let files = get_files(path).unwrap();
    let mut post_ids = Vec::new();
    let mut artists = Vec::new();
    let total_files_num = files.len();

    for (count, file) in files.iter().enumerate() {
        let mut retries = 0;
        let mut delay = Duration::from_millis(100);
        println!("Uploading {} | {}/{}", file.to_string_lossy(), count + 1, total_files_num);

        loop {
            match post_utils::create_post(client, &file).await {
                Ok((post_id, artist)) => {
                    post_ids.push(post_id);
                    artists.push(artist);
                    println!("Finished {}", file.to_string_lossy());

                    if config.settings.delete_files_in_progress {
                        match delete_file(file) {
                            Ok(_) => println!("File deleted successfully."),
                            Err(e) => eprintln!("Error deleting file: {}", e),
                        }
                    }
                    break;
                }
                Err(e) if retries < config.settings.retry_attempts => {
                    eprintln!(
                        "Error uploading post for file {}: {}. Retrying... (Attempt {}/{})",
                        file.display(),
                        e,
                        retries + 1,
                        config.settings.retry_attempts
                    );
                    retries += 1;
                    sleep(delay).await;
                    delay += Duration::from_millis(config.settings.timeout);
                }
                Err(e) => {
                    if config.settings.retry_attempts > 1 {
                        eprintln!(
                            "Error uploading post for file {}: {}. Max retries reached.",
                            file.display(),
                            e
                        );
                    }

                    if config.settings.skip_on_error {
                        eprintln!("Skipping file {} due to error.", file.display());
                        break; // Skip to the next file
                    } else {
                        return Err(e); // Ensure the function exits with an error
                    }
                }
            }
        }

        // Wait before uploading the next file
        sleep(Duration::from_millis(config.settings.timeout)).await;
    }

    println!("Finished");
    if config.settings.delete_folder {
        match delete_folder(path) {
            Ok(_) => println!("Folder deleted successfully."),
            Err(e) => eprintln!("Error deleting folder: {}", e),
        }
    }

    Ok(post_ids)
}

async fn merge_posts(client: &SzurubooruClient, path: &str, config: Config) -> SzurubooruResult<Vec<u32>> {
    let posts_ids: Vec<(u32, u32)> = post_utils::read_number_pairs(path)?;
    let merged_ids: Vec<u32> = posts_ids.iter().map(|(_, b)| *b).collect();    

    let progress_bar = ProgressBar::new(posts_ids.len() as u64);

    // Define multiple progress styles with different bar colors
    let success_style = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.green/green}] {pos}/{len} ({eta})")
        .expect("Failed to set success progress bar style")
        .progress_chars("#>-");

    let error_style = ProgressStyle::default_bar()
        .template("{spinner:.red} [{elapsed_precise}] [{bar:40.red/red}] {pos}/{len} ({eta})")
        .expect("Failed to set error progress bar style")
        .progress_chars("#>-");

    let default_style = ProgressStyle::default_bar()
        .template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .expect("Failed to set default progress bar style")
        .progress_chars("#>-");

    progress_bar.set_style(default_style.clone());

    for (remove_post, merge_to_post) in posts_ids {
        progress_bar.inc(1);

        let result = async {
            let remove_post_version = client
                .request()
                .get_post(remove_post)
                .await?
                .version
                .ok_or_else(|| SzurubooruClientError::IOError(Error::new(ErrorKind::InvalidData, "Missing remove_post version.")))?;

            let merge_to_version = client
                .request()
                .get_post(merge_to_post)
                .await?
                .version
                .ok_or_else(|| SzurubooruClientError::IOError(Error::new(ErrorKind::InvalidData, "Missing merge_to_post version.")))?;

            let merge = MergePost {
                remove_post_version,
                remove_post,
                merge_to_version,
                merge_to_post,
                replace_post_content: false,
            };

            client.request().merge_post(&merge).await
        };

        if let Err(e) = result.await {
            progress_bar.set_style(error_style.clone()); // Switch to red style on error
            progress_bar.set_message("Error encountered.");
            if !config.settings.skip_on_error {
                progress_bar.finish_with_message("Error encountered.");
                return Err(e);
            }
        } else {
            progress_bar.set_style(success_style.clone()); // Switch to green style on success
            progress_bar.set_message("Success");
        }

        sleep(Duration::from_millis(config.settings.timeout)).await;

        // Reset to default style for the next iteration
        progress_bar.set_style(default_style.clone());
    }

    progress_bar.finish_with_message("Merge complete.");
    Ok(merged_ids)
}



fn delete_folder(path: &str) -> io::Result<()> {
    fs::remove_dir(path)
}


fn delete_file(path: &PathBuf) -> io::Result<()> {
    // Remove the original file
    fs::remove_file(path)?;

    // Construct paths for the associated files
    if let Some(stem) = path.file_name().and_then(|f| f.to_str()) {
        let parent = path.parent().unwrap_or_else(|| path);
        
        // Build `img.png.json` and `img.png.txt` instead of `img.json` and `img.txt`
        let json_path = parent.join(format!("{}.json", stem));
        if json_path.exists() {
            fs::remove_file(&json_path)?;
        }

        let txt_path = parent.join(format!("{}.txt", stem));
        if txt_path.exists() {
            fs::remove_file(&txt_path)?;
        }
    }

    Ok(())
}

async fn upload_pool(client: &SzurubooruClient, path: &str) {
    // match post_utils::get_sorted_filenames(path) {
    //     Ok(filenames) => {
    //         match upload_posts(client, path).await {
    //             Ok(post_ids) => {
    //                 // Create a new pool using the post IDs
    //                 let pool_name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
    //                 let create_pool = CreateUpdatePoolBuilder::default()
    //                     .names(vec![pool_name])
    //                     .posts(Some(post_ids))
    //                     .build()
    //                     .unwrap();

    //                 match client.create_pool(&create_pool).await {
    //                     Ok(_) => println!("Pool created successfully"),
    //                     Err(e) => eprintln!("Error creating pool: {}", e),
    //                 }
    //             }
    //             Err(e) => {
    //                 eprintln!("Error uploading posts for pool: {}", e);
    //             }
    //         }
    //     }
    //     Err(e) => {
    //         eprintln!("Error getting sorted filenames: {}", e);
    //     }
    // }
    todo!()
}

#[derive(Deserialize, Debug)]
struct Config {
    server: ServerConfig,
    auth: AuthConfig,
    settings: SettingsConfig,
}

#[derive(Deserialize, Debug)]
struct ServerConfig {
    url: String,
}

#[derive(Deserialize, Debug)]
struct AuthConfig {
    username: String,
    token: String, // Username and token only, no password
}

#[derive(Deserialize, Debug)]
struct SettingsConfig {
    timeout: u64,
    retry_attempts: u8,
    log_level: String,
    skip_on_error: bool,
    delete_files_in_progress: bool,
    delete_folder: bool,
}

fn load_or_create_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_path = "config.toml";

    // Check if the file exists
    if !Path::new(config_path).exists() {
        // Prompt the user
        println!("The configuration file 'config.toml' does not exist. Would you like to create one? (yes/y/no)");

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        // Accept "yes" or "y" (case-insensitive)
        if input.trim().eq_ignore_ascii_case("yes") || input.trim().eq_ignore_ascii_case("y") {
            // Default configuration
            let default_config = r#"
[server]
url = "https://your-server-url.com"

[auth]
username = "your_username"
token = "your_auth_token"

[settings]
timeout = 30
retry_attempts = 3
skip_on_error = false
log_level = "info"
delete_files_in_progress = true
delete_folder = false
"#;

            // Write default config to file
            fs::write(config_path, default_config)?;
            println!("Default 'config.toml' file has been created. Exiting program...");
            std::process::exit(0);
        } else {
            println!("No configuration file created. Exiting...");
            std::process::exit(1);
        }
    }

    // At this point, the file exists, so load it
    let config_data = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&config_data)?;
    Ok(config)
}

