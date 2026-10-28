# A stand-in image for images.yml's push test, not a product image. It holds the least an image can
# hold and still be saved: the source label every image of this repository carries, and one tiny
# layer for it to sit on. `FROM scratch` with nothing else is an image with no layers at all, and
# `docker save` refuses one of those ("empty export - not implemented"), which left the push test
# loading an empty archive. The test builds one per Linux platform with `docker buildx build
# --platform`, and there is nothing in it to run, so no emulation is needed. The architecture check
# in push-image.sh reads the platform buildx records here.
FROM scratch

LABEL org.opencontainers.image.source="https://github.com/sanamhub/sluice"

# The layer: the Dockerfile itself, the only file the build context needs, copied in so the image
# has one layer of a few hundred bytes that does nothing.
COPY Dockerfile /sluice-push-test/Dockerfile
