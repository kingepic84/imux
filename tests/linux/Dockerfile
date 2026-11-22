FROM rust:1.94-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends util-linux procps lsof zsh fish ksh python3 perl rsync git ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY ca/ /usr/local/share/ca-certificates/
RUN update-ca-certificates && rustup component add clippy

RUN useradd --create-home --uid 1000 --shell /bin/bash dev \
 && mkdir -p /work/target /home/dev/.cargo \
 && chown -R dev:dev /work /home/dev

USER dev
RUN touch /home/dev/.zshrc
ENV CARGO_HOME=/home/dev/.cargo SHELL=/bin/zsh LANG=C.UTF-8 TERM=xterm-256color COLORTERM=truecolor
WORKDIR /work
