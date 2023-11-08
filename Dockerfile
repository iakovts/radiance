FROM ubuntu:latest

# SHELL ["/bin/sh", "-c"]
WORKDIR /app
# COPY . .
RUN apt update && apt install software-properties-common -y
RUN apt install git cmake qtbase5-dev qtdeclarative5-dev qtscript5-dev qtquickcontrols2-5-dev \
    qt5-image-formats-plugins qtscript5-dev libfftw3-dev libsamplerate0-dev \ 
     libasound2-dev libmpv-dev libdrm-dev libgl1-mesa-dev libportaudio2 portaudio19-dev autoconf libtool xvfb alsa-tools \ 
     alsa-utils libsndfile1-dev g++ -y 
RUN git clone https://github.com/thestk/rtmidi 
WORKDIR /app/rtmidi
RUN  git checkout 88e53b9
RUN /bin/bash -c "./autogen.sh"
RUN make && make install
WORKDIR /app
RUN git clone https://github.com/iakovts/radiance
WORKDIR /app/radiance
RUN git submodule update --init
WORKDIR /app/radiance/build
RUN cmake .. -DCMAKE_SHARED_LIBS=OFF # -DCMAKE_PREFIX_PATH=/opt/qt59/ && make -j$(($(nproc) - 1))
RUN make

