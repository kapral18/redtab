use super::*;

#[test]
fn when_membership_finishes_it_should_invalidate_reads_started_during_the_write() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let namespace = "membership-fixture";
    let read = Job::Subscriptions { generation: 1 };
    let job = Job::Membership {
        generation: 2,
        target: crate::model::MembershipTarget::Community("armenia".into()),
        join: true,
    };
    let (key, _) = read.cache_key(namespace).unwrap();
    let (tx, _rx) = sync_channel(8);
    for result in [Ok(()), Err("ambiguous write failure".into())] {
        let write_start = cache.reserve_request().unwrap();
        let late_read = cache.reserve_request().unwrap();
        cache
            .store(
                &key,
                br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                crate::app::now(),
                late_read,
            )
            .unwrap();
        cache_result(
            Msg::Membership {
                generation: 2,
                result,
            },
            &job,
            Some(&cache),
            namespace,
            &tx,
            Source {
                order: Some(write_start),
                ..Source::default()
            },
        );
        cache
            .store(
                &key,
                br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                crate::app::now(),
                late_read,
            )
            .unwrap();
        let entry = cache.load(&key, crate::app::now()).unwrap();
        assert!(
            entry.bytes.is_empty(),
            "a late read cannot resurrect pre-write membership"
        );
        assert!(read.cached_message(&entry.bytes).is_none());
        assert!(entry.order.sequence > late_read.sequence);
        let confirmed = cache.reserve_request().unwrap();
        cache
                .store(
                    &key,
                    br#"[{"community":{"name":"armenia","icon":"unknown"}},{"community":{"name":"rust","icon":"unknown"}}]"#,
                    crate::app::now(),
                    confirmed,
                )
                .unwrap();
        cache
            .store(
                &key,
                br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                crate::app::now(),
                late_read,
            )
            .unwrap();
        assert_eq!(
                cache.load(&key, crate::app::now()).unwrap().bytes,
                br#"[{"community":{"name":"armenia","icon":"unknown"}},{"community":{"name":"rust","icon":"unknown"}}]"#
            );
    }
}
