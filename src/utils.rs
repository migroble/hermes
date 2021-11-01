pub mod git {
    use anyhow::{Context, Result};
    use git2::{
        build::{CheckoutBuilder, RepoBuilder},
        Cred, FetchOptions, RebaseOptions, RemoteCallbacks, Repository,
    };
    use std::path::{Path, PathBuf};

    pub struct KeyPair {
        pub public: PathBuf,
        pub private: PathBuf,
    }

    fn fetch_options(ssh_key: &KeyPair) -> FetchOptions {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(move |_url, username_from_url, _allowed_types| {
            Cred::ssh_key(
                username_from_url.unwrap_or("git"),
                Some(&ssh_key.public),
                &ssh_key.private,
                None,
            )
        });
        let mut fo = FetchOptions::new();
        fo.remote_callbacks(callbacks);
        fo
    }

    pub fn clone(ssh_key: &KeyPair, url: &str, path: &Path) -> Result<bool> {
        let mut builder = RepoBuilder::new();
        builder.fetch_options(fetch_options(ssh_key));

        builder
            .clone(url, path)
            .context(format!("unable to clone {}", url))?;

        Ok(true)
    }

    pub fn fetch(ssh_key: &KeyPair, url: &str, path: &Path) -> Result<bool> {
        let repo = Repository::open(path)?;
        let mut remote = repo.find_remote("origin")?;
        remote
            .fetch(&["main"], Some(&mut fetch_options(ssh_key)), None)
            .context(format!("unable to fetch {}", url))?;
        // Since we just fetched, we are guaranteed to have a FETCH_HEAD, so we can unwrap safely
        let fetchhead = repo
            .annotated_commit_from_fetchhead(
                "main",
                url,
                &repo.refname_to_id("FETCH_HEAD").unwrap(),
            )
            .unwrap();

        let mut cb = CheckoutBuilder::new();
        cb.force();
        let mut ro = RebaseOptions::new();
        ro.checkout_options(cb);

        let rebase = repo
            .rebase(None, Some(&fetchhead), None, Some(&mut ro))
            .context(format!("unable to rebase {:#?}", path))?;
        if rebase.len() == 0 {
            Ok(false)
        } else {
            for _ in rebase {}
            Ok(true)
        }
    }

    pub fn clone_or_fetch_repo(ssh_key: &KeyPair, url: &str, path: &Path) -> Result<bool> {
        if path.is_dir() {
            fetch(ssh_key, url, path)
        } else {
            clone(ssh_key, url, path)
        }
    }
}

pub mod docker {
    use crate::config::Config;
    use anyhow::{Context, Result};
    use bollard::{
        container::{Config as ContainerConfig, CreateContainerOptions, StartContainerOptions},
        image::BuildImageOptions,
        models::HostConfig,
        Docker,
    };
    use futures::stream::StreamExt;
    use std::path::Path;
    use tar::Builder;

    pub async fn build_image(docker: &Docker, name: &str, repo_path: &Path) -> Result<()> {
        let mut tar_file = Builder::new(Vec::new());
        tar_file.append_dir_all(".", repo_path).context(format!(
            "unable to append files in {:#?} to tar file",
            repo_path
        ))?;
        // Writing to a Vec is infallible, we can unwrap safely
        let tar_file = tar_file.into_inner().unwrap();

        let mut stream = docker.build_image(
            BuildImageOptions {
                dockerfile: "Dockerfile",
                t: name,
                q: true,
                ..Default::default()
            },
            None,
            Some(tar_file.into()),
        );

        while stream.next().await.is_some() {}

        Ok(())
    }

    pub async fn stop_container(docker: &Docker, config: &Config) -> Result<()> {
        docker
            .stop_container(&config.name, None)
            .await
            .context(format!(
                "unable to stop Docker container {:#?}",
                config.name
            ))?;

        Ok(())
    }

    pub async fn run_container(docker: &Docker, config: Config) -> Result<()> {
        let co = CreateContainerOptions {
            name: config.name.clone(),
        };
        let cc = ContainerConfig {
            env: config.env,
            host_config: Some(HostConfig {
                binds: config.volumes,
                port_bindings: None, // TODO: add port bindings to config
                restart_policy: config.restart,
                ..Default::default()
            }),
            ..Default::default()
        };

        docker
            .create_container(Some(co), cc)
            .await
            .context(format!(
                "unable to create Docker container {:#?}",
                config.name
            ))?;
        docker
            .start_container(&config.name, None::<StartContainerOptions<String>>)
            .await
            .context(format!(
                "unable to start Docker container {:#?}",
                config.name
            ))?;

        Ok(())
    }
}
