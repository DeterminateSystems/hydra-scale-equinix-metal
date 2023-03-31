# Equinix Metal autoscaler for Hydra

This tool will query a Hydra instance for queue statistics and create
or destroy Equinix Metal instances according to load. It is used to
scale up and down capacity for hydra.nixos.org.

## Usage

Currently we have quite a few assumptions that only really apply to
the NixOS Hydra instance. Check out the source if you're interested in
running this yourself, because you'll probably need to do that anyway
;)

It relies on (this list may not be exhaustive):

 - [hydra-packet-importer](https://github.com/NixOS/nixos-org-configurations/tree/master/hydra-packet-importer) running to get the instances into Hydra's builder list

 - A Prometheus instance being set up to scrape Hydra, in order to determine what's currently running on a given builder

 - [nix-netboot-serve](https://github.com/DeterminateSystems/nix-netboot-serve/) or some other mechanism being set up to serve boot configs for the build machines
