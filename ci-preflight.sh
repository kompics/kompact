# Executes the core/test-feature-matrix.sh inside a Docker container with an Interactive-shell
docker run --rm -it \
-v $(pwd):/home/user/kompact \
--user user:root \
kompact:0.1 \
/bin/bash -c "cd /home/user/; source .bash_profile; rustup default stable; cd kompact/core; ./test-feature-matrix.sh"
#docker image build -t kompact:0.1 .
#docker container run --detach --name k kompact:0.1