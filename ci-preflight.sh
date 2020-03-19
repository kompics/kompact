# Executes the core/test-feature-matrix.sh inside a Docker container with an Interactive-shell
# This is equivalent to the CI tests
# Beta
docker run --rm -it \
-v $(pwd):/home/user/kompact \
--user user:root \
kompact:0.1 \
/bin/bash -c "cd /home/user/; source .bash_profile; rustup default beta; cd kompact/core; ./test-feature-matrix.sh"
# Nightly
docker run --rm -it \
-v $(pwd):/home/user/kompact \
--user user:root \
kompact:0.1 \
/bin/bash -c "cd /home/user/; source .bash_profile; rustup default nightly; cd kompact/core; ./test-feature-matrix.sh"
# Stable
docker run --rm -it \
-v $(pwd):/home/user/kompact \
--user user:root \
kompact:0.1 \
/bin/bash -c "cd /home/user/; source .bash_profile; rustup default stable; cd kompact/core; ./test-feature-matrix.sh"