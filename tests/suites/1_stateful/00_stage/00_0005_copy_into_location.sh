#!/usr/bin/env bash

CURDIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$CURDIR"/../../../shell_env.sh


stmt "create or replace connection c_00_0005 storage_type='s3' access_key_id = 'minioadmin'  endpoint_url = 'http://127.0.0.1:9900' secret_access_key = 'minioadmin'"

query "copy into 's3://testbucket/c_00_0005/ab de/f' connection=(connection_name='c_00_0005') from (select 1) detailed_output=true use_raw_path=true single=true overwrite=true"