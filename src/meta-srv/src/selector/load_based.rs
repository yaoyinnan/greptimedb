// Copyright 2023 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;

use api::v1::meta::Peer;
use common_telemetry::warn;
use common_time::util as time_util;

use crate::cluster::MetaPeerClient;
use crate::error::Result;
use crate::keys::{LeaseKey, LeaseValue, StatKey};
use crate::lease;
use crate::metasrv::SelectorContext;
use crate::selector::{Namespace, Selector};

const MAX_REGION_NUMBER: u64 = u64::MAX;

pub struct LoadBasedSelector {
    pub meta_peer_client: MetaPeerClient,
}

#[async_trait::async_trait]
impl Selector for LoadBasedSelector {
    type Context = SelectorContext;
    type Output = Vec<Peer>;

    async fn select(&self, ns: Namespace, ctx: &Self::Context) -> Result<Self::Output> {
        // get alive datanodes
        let lease_filter = |_: &LeaseKey, v: &LeaseValue| {
            time_util::current_time_millis() - v.timestamp_millis < ctx.datanode_lease_secs * 1000
        };
        let lease_kvs: HashMap<LeaseKey, LeaseValue> =
            lease::alive_datanodes(ns, &ctx.kv_store, lease_filter)
                .await?
                .into_iter()
                .collect();

        if lease_kvs.is_empty() {
            return Ok(vec![]);
        }

        // get stats of alive datanodes
        let stat_keys: Vec<StatKey> = lease_kvs
            .keys()
            .map(|k| StatKey {
                cluster_id: k.cluster_id,
                node_id: k.node_id,
            })
            .collect();
        let stat_kvs = self.meta_peer_client.get_dn_stat_kvs(stat_keys).await?;

        let mut tuples: Vec<(LeaseKey, LeaseValue, u64)> = lease_kvs
            .into_iter()
            .map(|(lease_k, lease_v)| {
                let stat_key: StatKey = (&lease_k).into();

                let region_num = match stat_kvs
                    .get(&stat_key)
                    .and_then(|stat_val| stat_val.region_num())
                {
                    Some(region_num) => region_num,
                    None => {
                        warn!("Failed to get stat_val by stat_key {:?}", stat_key);
                        MAX_REGION_NUMBER
                    }
                };

                (lease_k, lease_v, region_num)
            })
            .collect();

        // sort the datanodes according to the number of regions
        tuples.sort_by(|a, b| a.2.cmp(&b.2));

        Ok(tuples
            .into_iter()
            .map(|(stat_key, lease_val, _)| Peer {
                id: stat_key.node_id,
                addr: lease_val.node_addr,
            })
            .collect())
    }
}
