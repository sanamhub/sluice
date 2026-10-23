# A stand-in image for images.yml's push test, not a product image. It holds the least a Docker
# archive can: one empty config and the source label every image of this repository carries. The
# test builds one per Linux platform with `docker buildx build --platform`, and there is nothing in
# it to run, so no emulation is needed. The architecture check in push-image.sh reads the platform
# buildx records here.
FROM scratch

LABEL org.opencontainers.image.source="https://github.com/sanamhub/sluice"
