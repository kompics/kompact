### This creates a docker file which you can use to locally build the image used by ci-preflight.sh to run ci equivalent containerized tests.
#
# Base image with rustup pre-installed
FROM liuchong/rustup:stable
# Can't execute tests as root
RUN useradd -ms /bin/bash user
RUN usermod -aG sudo user
RUN usermod -aG root user
# Need a user password and this needs to execute in bash
RUN /bin/bash -c "echo -e 'password\npassword' | passwd user"
RUN echo "PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" >> /home/user/.bash_profile
RUN chown user:root /home/user
# Rustup directory:
RUN chmod g+rwx /root/
# Add some tools for automated installs/deployments
RUN apt update -y
RUN apt upgrade -y
RUN apt install wget -y
RUN apt install unzip -y
RUN apt install sudo -y