// post_utils.rs

use errors::SzurubooruClientError;
use models::{CreateUpdatePost, PostSafety};
use serde_json::Value;
use tokio::fs::File;
use tokio::io::BufReader;
use std::collections::HashSet;
use std::fs;
use std::hash::Hash;
use std::io::{Read, self, BufRead, Error, ErrorKind};
use std::path::{Path, PathBuf};
use szurubooru_client::*;

const MEDIA_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "mp4", "webm", "gif", "swf", "webp"];

pub async fn create_post(
    client: &SzurubooruClient,
    file_path: &PathBuf,
) -> SzurubooruResult<(u32, Option<String>)> {
    let search_result = client
        .request()
        .reverse_search_file_path(file_path.clone())
        .await?;
    let (exact_post, similar_posts) = (search_result.exact_post, search_result.similar_posts);
    let file_token = client
        .request()
        .upload_temporary_file_from_path(file_path.clone())
        .await?;
    let (mut post, creator) = make_post_with_metadata(file_token.token, file_path.clone())?;
    let artist = if creator.is_some() { creator } else { None };
    if !similar_posts.is_empty() {
        let similar_posts_ids: Vec<u32> = similar_posts
            .into_iter()
            .filter(|similar_post| similar_post.distance >= 0.75)
            .map(|similar_post| similar_post.post.id.unwrap())
            .collect();
        post.relations = Some(similar_posts_ids);
    }
    if exact_post.is_some() {
        let exact_post = exact_post.unwrap();
        let exact_tags: Option<Vec<String>> = exact_post.tags.map(|tags_vec| {
            tags_vec
                .into_iter()
                .filter_map(|tag_resource| tag_resource.names.first().cloned())
                .collect()
        });
        let exact_relations: Option<Vec<u32>> = exact_post.relations.map(|tags_vec| {
            tags_vec
                .into_iter()
                .filter_map(|post_resource| Some(post_resource.id))
                .collect()
        });

        post = CreateUpdatePost {
            version: exact_post.version,
            tags: merge_vecs_unique(&exact_tags, &post.tags),
            safety: if post.safety.is_some() {
                post.safety
            } else if exact_post.safety.is_some() {
                exact_post.safety
            } else {
                Some(PostSafety::Unsafe)
            },
            source: merge_source(exact_post.source, post.source),
            relations: merge_vecs_unique(&exact_relations, &post.relations),
            notes: None,
            flags: None,
            content_url: None,
            content_token: None,
            anonymous: Some(false),
        };
        return match client
            .request()
            .update_post(exact_post.id.unwrap(), &post)
            .await
        {
            Ok(post) => Ok((post.id.unwrap(), artist)),
            Err(e) => Err(e),
        };
    }

    match client
        .request()
        .create_post_from_file_path(file_path.clone(), Option::<PathBuf>::None, &post)
        .await
    {
        Ok(post) => Ok((post.id.unwrap(), artist)),
        Err(err) => Err(err),
    }
}

fn make_post_with_metadata(
    token: String,
    file_path: PathBuf,
) -> Result<(CreateUpdatePost, Option<String>), SzurubooruClientError> {
    let mut post = CreateUpdatePost {
        version: None,
        tags: None,
        safety: None,
        source: None,
        relations: None,
        notes: None,
        flags: None,
        content_url: None,
        content_token: Some(token),
        anonymous: Some(false), //possible cli arg
    };

    let artist: Option<String> = None;

    // Check for TXT file for tags
    let txt_path = {
        let txt_file_name = format!("{}.txt", file_path.file_name().unwrap().to_string_lossy());
        file_path.with_file_name(txt_file_name)
    };
    if txt_path.exists() {
        println!("Found txt");
        let mut content = String::new();
        let _ = fs::File::open(&txt_path)
            .map_err(|e| SzurubooruClientError::IOError(e))?
            .read_to_string(&mut content);
        let tags_vec: Vec<String>= content
        .trim()
        .split('\n')
        .map(|s| s.trim().replace(" ", "_").to_string())
        .collect();
        post.tags = Some(tags_vec.clone());
        println!("Tags: {}", tags_vec.join(","));
    }

    // Check for JSON file for additional metadata
    let json_path = {
        let json_file_name = format!("{}.json", file_path.file_name().unwrap().to_string_lossy());
        file_path.with_file_name(json_file_name)
    };

    if json_path.exists() {
        println!("Found json");
        let mut content = String::new();
        let _ = fs::File::open(&json_path)
            .map_err(|e| SzurubooruClientError::IOError(e))?
            .read_to_string(&mut content);
        let json_data: Value = serde_json::from_str(&content).map_err(|e| {
            SzurubooruClientError::ResponseParsingError(e, "Error parsing sidecar".to_string())
        })?;

        // Extract source and url, appending them if necessary
        if let Some(source) = json_data.get("source").and_then(|s| s.as_str()) {
            post.source = Some(source.to_string());
        }

        if let Some(url) = json_data.get("url").and_then(|u| u.as_str()) {
            post.source = Some(match &post.source {
                Some(existing_source) => format!("{}\n{}", existing_source, url),
                none => url.to_string(),
            });
        }

        let website = json_data
        .get("category")
        .unwrap()
        .as_str()
        .unwrap_or_default();
        println!("Website: {}",website.to_string());

        let tags_vec: Option<Vec<String>> = match website {
            "art.mobius.social" | "sankaku" | "danbooru" => {
                // Handle tags as an array
                json_data.get("tags").and_then(|tags| tags.as_array()).map(|tags_array| {
                    tags_array
                        .iter()
                        .filter_map(|tag| tag.as_str().map(String::from))
                        .map(|s| s.to_lowercase().replace(' ', "_"))
                        .collect()
                })
            },
            "rule34" | "safebooru" => {
                // Handle tags as a space-separated string
                json_data.get("tags").and_then(|tags| tags.as_str()).map(|tags_str| {
                    tags_str
                        .split_whitespace()
                        .map(|tag| tag.to_string())
                        .collect()
                })
            },
            _ => {
                // Default case for comma-separated string or other unknown formats
                json_data.get("tags").and_then(|tags| tags.as_str()).map(|tags_str| {
                    tags_str
                        .split(',')
                        .map(|tag| tag.trim().to_string())
                        .collect()
                })
            }
        };
    
        if let Some(tags) = tags_vec {
            println!("Tags: {}", tags.join(", "));
            post.tags = Some(tags);
        } else {
            println!("No tags found for {}", website);
            post.tags = None;
        }

        // Extract username and add as a tag
        if let Some(username) = json_data.get("username") {
            if let Some(username_str) = username.as_str() {
                let tags_vec = post.tags.get_or_insert_with(Vec::new);
                tags_vec.push(format!("creator:{}", username_str));
            }
        }

        // Extract safety (leave as `None` if not found)
        if let Some(safety_str) = json_data
            .get("safety")
            .and_then(|s| s.as_str())
            .or_else(|| json_data.get("rating").and_then(|r| r.as_str()))
        {
            post.safety = match safety_str.to_lowercase().as_str() {
                "safe" | "s" => Some(PostSafety::Safe),
                "sketchy" | "questionable" | "q" => Some(PostSafety::Sketchy),
                "unsafe" | "explicit" | "e" => Some(PostSafety::Unsafe),
                other => {
                    println!("Unrecognized safety/rating found: {}", other);
                    None
                }
            };
        }
    }

    if post.safety.is_none() {
        post.safety = Some(PostSafety::Unsafe);
    }

    Ok((post, artist))
}

fn merge_source(opt1: Option<String>, opt2: Option<String>) -> Option<String> {
    let mut unique_lines = HashSet::new();

    // Collect lines from both options, if they exist
    if let Some(s1) = opt1 {
        unique_lines.extend(s1.lines().map(String::from));
    }
    if let Some(s2) = opt2 {
        unique_lines.extend(s2.lines().map(String::from));
    }

    // If there are any unique lines, join them with newline; otherwise, return None
    if unique_lines.is_empty() {
        None
    } else {
        Some(unique_lines.into_iter().collect::<Vec<_>>().join("\n"))
    }
}

fn merge_vecs_unique<T>(vec1: &Option<Vec<T>>, vec2: &Option<Vec<T>>) -> Option<Vec<T>>
where
    T: Eq + Hash + Clone,
{
    let mut unique_tags = HashSet::new();

    if let Some(t1) = vec1 {
        unique_tags.extend(t1.iter().cloned());
    }

    if let Some(t2) = vec2 {
        unique_tags.extend(t2.iter().cloned());
    }

    if !unique_tags.is_empty() {
        Some(unique_tags.into_iter().collect())
    } else {
        None
    }
}

pub fn get_sorted_filenames(path: &str) -> SzurubooruResult<Vec<String>> {
    let mut files = get_files(path)?;
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    Ok(files
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

pub fn get_files(path: &str) -> Result<Vec<PathBuf>, SzurubooruClientError> {
    let mut post_files = Vec::new();
    let dir = Path::new(path);

    if dir.is_dir() {
        for entry in fs::read_dir(dir).map_err(|e| SzurubooruClientError::IOError(e))? {
            let path = entry.map_err(|e| SzurubooruClientError::IOError(e))?.path();

            if path.is_file() {
                if let Some(extension) = path.extension() {
                    if let Some(ext_str) = extension.to_str() {
                        if MEDIA_EXTENSIONS.contains(&ext_str.to_lowercase().as_str()) {
                            post_files.push(path);
                        }
                    }
                }
            }
        }
    } else {
        let dir_error: std::io::Error = Error::new(std::io::ErrorKind::Other, "Not a directory");
        return Err(SzurubooruClientError::IOError(dir_error));
    }

    Ok(post_files)
}

pub fn read_number_pairs(file_path: &str) -> Result<Vec<(u32, u32)>, SzurubooruClientError> {
    let mut number_pairs = Vec::new();
    let path = Path::new(file_path);

    if path.is_file() {
        let file = fs::File::open(path).map_err(|e| SzurubooruClientError::IOError(e))?;
        let reader = io::BufReader::new(file);

        for line in reader.lines() {
            let line = line.map_err(|e| SzurubooruClientError::IOError(e))?;
            let numbers: Vec<&str> = line.split_whitespace().collect();

            if numbers.len() == 2 {
                let first = numbers[0].parse::<u32>().map_err(|_| {
                    SzurubooruClientError::IOError(Error::new(ErrorKind::InvalidData, "Failed to parse the first number."))
                })?;
                let second = numbers[1].parse::<u32>().map_err(|_| {
                    SzurubooruClientError::IOError(Error::new(ErrorKind::InvalidData, "Failed to parse the second number."))
                })?;
                number_pairs.push((first, second));
            } else {
                return Err(SzurubooruClientError::IOError(Error::new(
                    ErrorKind::InvalidData,
                    "Each line must contain exactly two numbers."
                )));
            }
        }
    } else {
        let dir_error: std::io::Error = Error::new(std::io::ErrorKind::Other, "Provided path is not a file");
        return Err(SzurubooruClientError::IOError(dir_error));
    }

    Ok(number_pairs)
}
