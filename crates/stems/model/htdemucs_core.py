"""Wrapper exporting the HTDemucs network without its STFT/iSTFT.

inputs:  mix [1,2,343980] (normalized time signal), mag [1,4,2048,336] (complex-as-channels spectrogram)
outputs: spec_out [1,4,4,2048,336] (separated spectrograms, de-normalized), time_out [1,4,2,343980]
The body is HTDemucs.forward (demucs 4.1.0, demucs/htdemucs.py) from the normalization of the
inputs to the de-normalization of both branches, unchanged.
"""
import torch

class Core(torch.nn.Module):
    def __init__(s,m): super().__init__(); s.m=m
    def forward(s,mix,mag):
        m=s.m; x=mag; B,C,Fq,T=x.shape
        mean=x.mean(dim=(1,2,3),keepdim=True); std=x.std(dim=(1,2,3),keepdim=True)
        x=(x-mean)/(1e-5+std)
        xt=mix; meant=xt.mean(dim=(1,2),keepdim=True); stdt=xt.std(dim=(1,2),keepdim=True)
        xt=(xt-meant)/(1e-5+stdt)
        saved=[];saved_t=[];lengths=[];lengths_t=[]
        for idx,encode in enumerate(m.encoder):
            lengths.append(x.shape[-1]); inject=None
            if idx<len(m.tencoder):
                lengths_t.append(xt.shape[-1]); tenc=m.tencoder[idx]; xt=tenc(xt)
                if not tenc.empty: saved_t.append(xt)
                else: inject=xt
            x=encode(x,inject)
            if idx==0 and m.freq_emb is not None:
                frs=torch.arange(x.shape[-2]); emb=m.freq_emb(frs).t()[None,:,:,None].expand_as(x)
                x=x+m.freq_emb_scale*emb
            saved.append(x)
        if m.bottom_channels:
            b,c,f,t=x.shape; x=x.reshape(b,c,f*t); x=m.channel_upsampler(x); x=x.reshape(b,-1,f,t); xt=m.channel_upsampler_t(xt)
        x,xt=m.crosstransformer(x,xt)
        if m.bottom_channels:
            x=x.reshape(b,-1,f*t); x=m.channel_downsampler(x); x=x.reshape(b,-1,f,t); xt=m.channel_downsampler_t(xt)
        for idx,decode in enumerate(m.decoder):
            skip=saved.pop(-1); x,pre=decode(x,skip,lengths.pop(-1))
            offset=m.depth-len(m.tdecoder)
            if idx>=offset:
                tdec=m.tdecoder[idx-offset]; lt=lengths_t.pop(-1)
                if tdec.empty: pre=pre[:,:,0]; xt,_=tdec(pre,None,lt)
                else: xt,_=tdec(xt,saved_t.pop(-1),lt)
        S=len(m.sources)
        x=x.view(B,S,-1,Fq,T)*std[:,None]+mean[:,None]
        xt=xt.view(B,S,-1,mix.shape[-1])*stdt[:,None]+meant[:,None]
        return x,xt
