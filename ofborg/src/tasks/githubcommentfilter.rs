extern crate amqp;
extern crate env_logger;

use ofborg::ghevent;
use ofborg::acl;
use serde_json;

use hubcaps;
use ofborg::message::{Repo, Pr, buildjob, massrebuildjob};
use ofborg::worker;
use ofborg::commentparser;
use amqp::protocol::basic::{Deliver,BasicProperties};


pub struct GitHubCommentWorker {
    acl: acl::ACL,
    github: hubcaps::Github
}

impl GitHubCommentWorker {
    pub fn new(acl: acl::ACL, github: hubcaps::Github) -> GitHubCommentWorker {
        return GitHubCommentWorker{
            acl: acl,
            github: github,
        };
    }
}

impl worker::SimpleWorker for GitHubCommentWorker {
    type J = ghevent::IssueComment;

    fn msg_to_job(&self, _: &Deliver, _: &BasicProperties,
                  body: &Vec<u8>) -> Result<Self::J, String> {
        return match serde_json::from_slice(body) {
            Ok(e) => { Ok(e) }
            Err(e) => {
                println!("Failed to deserialize IsssueComment: {:?}", String::from_utf8(body.clone()));
                panic!("{:?}", e);
            }
        }
    }

    fn consumer(&self, job: &ghevent::IssueComment) -> worker::Actions {
        let instructions = commentparser::parse(&job.comment.body);
        if instructions == None {
            return vec![
                worker::Action::Ack
            ];
        }

        if !self.acl.can_build(&job.comment.user.login, &job.repository.full_name) {
            println!("ACL prohibits {} from building {:?} for {}",
                     job.comment.user.login,
                     instructions,
                     job.repository.full_name);
            return vec![
                worker::Action::Ack
            ];
        }

        println!("Got job: {:?}", job);

        let instructions = commentparser::parse(&job.comment.body);
        println!("Instructions: {:?}", instructions);

        let pr = self.github
            .repo(job.repository.owner.login.clone(), job.repository.name.clone())
            .pulls()
            .get(job.issue.number)
            .get();

        if let Err(x) = pr {
            info!("fetching PR {}#{} from GitHub yielded error {}",
                  job.repository.full_name,
                  job.issue.number,
                  x
            );
            return vec![
                worker::Action::Ack
            ];
        }

        let pr = pr.unwrap();

        let mut response: Vec<worker::Action> = vec![];
        if let Some(instructions) = instructions {
            for instruction in instructions {
                match instruction {
                    commentparser::Instruction::Build(attrs) => {
                        let msg = buildjob::BuildJob{
                            repo: Repo {
                                clone_url: job.repository.clone_url.clone(),
                                full_name: job.repository.full_name.clone(),
                                owner: job.repository.owner.login.clone(),
                                name: job.repository.name.clone(),
                            },
                            pr: Pr {
                                number: job.issue.number.clone(),
                                head_sha: pr.head.sha.clone(),
                                target_branch: Some(pr.base.commit_ref.clone())
                            },
                            attrs: attrs
                        };

                        let props = BasicProperties {
                            content_type: Some("application/json".to_owned()),
                            ..Default::default()
                        };

                        response.push(
                            worker::Action::Publish(worker::QueueMsg{
                                exchange: Some("build-jobs".to_owned()),
                                routing_key: None,
                                mandatory: true,
                                immediate: false,
                                properties: Some(props),
                                content: serde_json::to_string(&msg).unwrap().into_bytes()
                            })
                        );
                    }
                    commentparser::Instruction::Eval => {
                        let msg = massrebuildjob::MassRebuildJob{
                            repo: Repo {
                                clone_url: job.repository.clone_url.clone(),
                                full_name: job.repository.full_name.clone(),
                                owner: job.repository.owner.login.clone(),
                                name: job.repository.name.clone(),
                            },
                            pr: Pr {
                                number: job.issue.number.clone(),
                                head_sha: pr.head.sha.clone(),
                                target_branch: Some(pr.base.commit_ref.clone()),
                            },
                        };

                        let props = BasicProperties {
                            content_type: Some("application/json".to_owned()),
                            ..Default::default()
                        };

                        response.push(
                            worker::Action::Publish(worker::QueueMsg{
                                exchange: None,
                                routing_key: Some("mass-rebuild-check-jobs".to_owned()),
                                mandatory: true,
                                immediate: false,
                                properties: Some(props),
                                content: serde_json::to_string(&msg).unwrap().into_bytes()
                            })
                        );
                    }

                }
            }
        }

        response.push(worker::Action::Ack);
        return response;
    }
}