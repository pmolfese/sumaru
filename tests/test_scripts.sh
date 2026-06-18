#!/bin/bash

#draw an roi here
cargo run -- --surface testing/fs_lowres_std-lh.gii

#basic tests
cargo run -- --surface testing/lh.inflated.gii --roi testing/roi_1.lh.inflated.niml.roi
cargo run -- --surface testing/lh.inflated.gii --roi testing/test.roi1_roi2.niml.roi

#with stats
cargo run -- --surface testing/fs_lowres_std-lh.gii 
cargo run -- --surface testing/fs_lowres_std-lh.gii --overlay testing/ISC_lh_theta_neg.niml.dset


#test for both hemispheres
cargo run -- --spec testing/SUMA/sub-3_both.spec --sv testing/SUMA/sub-3_SurfVol.nii

#test for AFNI niml connection
cargo run -- --spec testing/SUMA/sub-3_both.spec --sv testing/SUMA/sub-3_SurfVol.nii --talk-afni

