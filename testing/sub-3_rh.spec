# delimits comments

# Creation information:
#     user    : molfesepj
#     date    : Wed Mar  5 17:38:28 EST 2025
#     machine : cn0025
#     pwd     : /data/molfesepj/SamWR/freesurfer/sub-3/SUMA
#     command : @SUMA_Make_Spec_FS -sid sub-3 -NIFTI

# define the group
        Group = sub-3

# define various States
        StateDef = smoothwm
        StateDef = pial
        StateDef = inflated
        StateDef = sphere
        StateDef = white
        StateDef = sphere.reg
        StateDef = inf_200

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.smoothwm.gii
        LocalDomainParent = SAME
        SurfaceState = smoothwm
        EmbedDimension = 3
        Anatomical = Y
        LabelDset = rh.aparc.a2009s.annot.niml.dset

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.pial.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = pial
        EmbedDimension = 3
        Anatomical = Y

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.inflated.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = inflated
        EmbedDimension = 3
        Anatomical = N

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.sphere.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = sphere
        EmbedDimension = 3
        Anatomical = N

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.white.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = white
        EmbedDimension = 3
        Anatomical = Y

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.sphere.reg.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = sphere.reg
        EmbedDimension = 3
        Anatomical = N

NewSurface
        SurfaceFormat = ASCII
        SurfaceType = GIFTI
        SurfaceName = rh.inf_200.gii
        LocalDomainParent = rh.smoothwm.gii
        SurfaceState = inf_200
        EmbedDimension = 3
        Anatomical = N

