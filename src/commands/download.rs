use super::{
    util,
    util::{get_organization, get_projects_dir, Client},
};
use crate::{
    interactive,
    io::{Io, PrintColor},
    progress_reporting,
    progress_reporting::ProgressBarManager,
};
use anyhow::Context;
use std::{path::Path, process::Command};
use tmc_langs::{ClientUpdateData, Course, DownloadResult};

// Downloads course exercises
// course_name as None will trigger interactive menu for selecting a course
// currentdir determines if course should be downloaded to current directory or central project directory
// Will run in privileged stage if needed on Windows.
pub fn download_or_update(
    io: &mut dyn Io,
    client: &mut dyn Client,
    course_name: Option<&str>,
    currentdir: bool,
) -> anyhow::Result<()> {
    get_organization().context("No organization found. Run 'tmc organization' first.")?;

    io.println("Fetching courses...", PrintColor::Normal)?;
    let courses = client.list_courses().context("Could not list courses.")?;

    let mut courses = courses
        .iter()
        .map(|course| client.get_course_details(course.id))
        .collect::<Result<Vec<_>, _>>()?;

    courses.sort_by(|a, b| {
        a.course
            .title
            .to_lowercase()
            .cmp(&b.course.title.to_lowercase())
    });

    let name_select = if let Some(course) = course_name {
        course
    } else {
        let course = get_course_name(
            courses
                .iter()
                .map(|course| course.course.title.clone())
                .collect(),
        )?;
        &courses
            .iter()
            .find(|c| c.course.title == course)
            .context("No course with the selected name was found")?
            .course
            .name
    };

    // Get course by name
    let course = match util::get_course_by_name(client, name_select)? {
        Some(course) => course,
        None => anyhow::bail!("Could not find course with that name"),
    };
    let pathbuf = if currentdir {
        std::env::current_dir()?
    } else {
        get_projects_dir()?
    };

    match download_exercises(&pathbuf, client, &course) {
        Ok(msg) => {
            io.println(&format!("\n{}", msg), PrintColor::Success)?;
            Ok(())
        }
        Err(err) => {
            let os = std::env::consts::OS;
            if os == "windows"
                && err
                    .chain()
                    .any(|e| e.to_string().contains("Failed to create file"))
            {
                io.println(
                    "Starting new cmd with administrator privileges...",
                    PrintColor::Normal,
                )?;
                let temp_file_path = get_projects_dir()?;
                let temp_file_path = temp_file_path.join("temp.txt");
                std::fs::write(
                    temp_file_path,
                    format!("{};{}", &pathbuf.display(), &course.name),
                )?;
                Command::new("cmd")
                    .args(&[
                        "/C",
                        "powershell",
                        "-Command",
                        "Start-Process",
                        "tmc.exe",
                        "elevateddownload",
                        "-Verb",
                        "RunAs",
                    ])
                    .spawn()
                    .context("launch failure")?;
                Ok(())
            } else {
                anyhow::bail!(err);
            }
        }
    }
}

pub fn get_course_name(courses: Vec<String>) -> anyhow::Result<String> {
    let result = interactive::interactive_list("Select your course:", courses)?;

    match result {
        Some(course) => {
            if course.is_empty() {
                anyhow::bail!("Could not find a course by the given title");
            } else {
                Ok(course)
            }
        }
        None => anyhow::bail!("Course selection was interrupted"),
    }
}

pub fn download_exercises(
    path: &Path,
    client: &mut dyn Client,
    course: &Course,
) -> anyhow::Result<String> {
    match client.get_course_exercises(course.id) {
        Ok(exercises) => {
            let exercise_ids: Vec<u32> = exercises
                .iter()
                .filter(|t| !t.disabled && t.unlocked)
                .map(|t| t.id)
                .collect();

            if exercise_ids.is_empty() {
                anyhow::bail!(format!(
                    "No valid exercises found for course '{}'",
                    course.title
                ));
            }

            // start manager for 1 event: tmc_langs::download_or_update_exercises
            let mut manager = ProgressBarManager::new(
                progress_reporting::get_default_style(),
                1,
                client.is_test_mode(),
            );
            manager.start::<ClientUpdateData>();

            let result = client.download_or_update_exercises(&exercise_ids, path);

            match result {
                Ok(download_result) => {
                    manager.join();
                    match download_result {
                        DownloadResult::Success {
                            downloaded: _,
                            skipped: _,
                        } => {
                            if client.is_test_mode() {
                                return Ok("Download was successful!".to_string());
                            }
                        }
                        DownloadResult::Failure {
                            downloaded,
                            skipped: _,
                            failed,
                        } => {
                            let mut res = String::from("");

                            for (id, messages) in failed {
                                res.push_str(&format!(
                                    "\nFailed to download exercise: '{}'",
                                    id.exercise_slug
                                ));
                                for message in messages {
                                    res.push_str(&format!("\n    with message: '{}'", message));
                                }
                            }

                            if !downloaded.is_empty() {
                                res.push_str(&format!(
                                    "\n\nSuccessful downloads saved to {}",
                                    path.display()
                                ));
                            }

                            anyhow::bail!(res);
                        }
                    }
                }
                Err(err) => {
                    manager.force_join();
                    anyhow::bail!(err);
                }
            }
        }
        Err(err) => anyhow::bail!(err),
    }

    Ok(format!(
        "Exercises downloaded successfully to {}",
        path.display()
    ))
}
pub fn elevated_download(io: &mut dyn Io, client: &mut dyn Client) -> anyhow::Result<()> {
    use std::io::prelude::*;
    let temp_file_path = get_projects_dir()?;
    let temp_file_path = temp_file_path.join("temp.txt");
    let mut file = std::fs::File::open(temp_file_path.clone())?;
    let mut params = String::new();
    file.read_to_string(&mut params)?;
    std::fs::remove_file(temp_file_path)?;
    let split = params.split(';');
    let vec = split.collect::<Vec<&str>>();
    let path = Path::new(vec[0]);
    let name_select = &vec[1];

    // Get course by name
    let course = match util::get_course_by_name(client, name_select)? {
        Some(course) => course,
        None => anyhow::bail!("Could not find course with that name"),
    };
    io.println("", PrintColor::Normal)?;
    let msg = download_exercises(path, client, &course)?;
    io.println(&msg, PrintColor::Success)?;
    pause()?;
    Ok(())
}

fn pause() -> anyhow::Result<()> {
    use std::{io, io::prelude::*};
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    write!(stdout, "Press any enter to continue...")?;
    stdout.flush()?;
    let mut s = String::new();
    stdin.read_line(&mut s)?;
    Ok(())
}
